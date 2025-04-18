use crate::command::VolumeCommand;

use std::{ffi::CString, os::unix::net::UnixStream};
use anyhow::Context;

use std::sync::mpsc;
use std::thread::sleep;
use std::time::Duration;

use log::{trace, debug, info, warn, error};

use pulseaudio::protocol;

fn connect_to_pulseaudio(seq: &mut u32, name: CString) -> anyhow::Result<(std::io::BufReader<UnixStream>, u16)> {
    // Find and connect to PulseAudio. The socket is usually in a well-known
    // location under XDG_RUNTIME_DIR.
    let socket_path = pulseaudio::socket_path_from_env().context("Failed to find pulseaudio socket")?;
    let mut sock = std::io::BufReader::new(UnixStream::connect(socket_path)?);

    // PulseAudio usually puts an authentication "cookie" in ~/.config/pulse/cookie.
    let cookie = pulseaudio::cookie_path_from_env()
        .and_then(|path| std::fs::read(path).ok())
        .unwrap_or_default();
    let auth = protocol::AuthParams {
        version: protocol::MAX_VERSION,
        supports_shm: false,
        supports_memfd: false,
        cookie,
    };

    // Write the auth "command" to the socket, and read the reply. The reply
    // contains the negotiated protocol version.
    protocol::write_command_message(
        sock.get_mut(),
        0,
        protocol::Command::Auth(auth),
        protocol::MAX_VERSION,
    )?;

    let (_, auth_reply) =
        protocol::read_reply_message::<protocol::AuthReply>(&mut sock, protocol::MAX_VERSION)?;
    let protocol_version = std::cmp::min(protocol::MAX_VERSION, auth_reply.version);

    // The next step is to set the client name.
    let mut props = protocol::Props::new();
    props.set(
        protocol::Prop::ApplicationName,
        name
    );
    protocol::write_command_message(
        sock.get_mut(),
        1,
        protocol::Command::SetClientName(props),
        protocol_version,
    )?;

    // The reply contains our client ID.
    let _ = protocol::read_reply_message::<protocol::SetClientNameReply>(&mut sock, protocol_version)?;

    *seq = 2;

    Ok((sock, protocol_version))
}

pub fn pulseaudio_listen(output_name: &str, volume_signal_tx: watch::WatchSender<VolumeCommand>) {
    let output_name = CString::new(output_name).unwrap();
    let connection_name = CString::new("mpd-sink-sync-listen").unwrap();
    let mut connection_failed = false;
    loop {
        trace!("Initiating PA listen connection...");
        let mut seq: u32 = 0;
        let (mut sock, protocol_version) = match connect_to_pulseaudio(&mut seq, connection_name.clone()) {
            Err(e) => {
                if !connection_failed {
                    warn!("Failed to connect to pulseaudio: {e:?}");
                    // Don't spam logs too much
                    connection_failed = true;
                }
                sleep(Duration::from_millis(2000));
                continue;
            }
            Ok(v) => v
        };
        connection_failed = true;

        // Query initial volume
        if let Err(e) = query_volume(
            &mut sock,
            &mut seq,
            protocol_version,
            protocol::command::GetSinkInfo { index: None, name: Some(output_name.clone()) },
            Some(output_name.clone()),
            volume_signal_tx.clone(),
        ) {
            error!("Failed to query initial PA volume: {e:?}");
            sleep(Duration::from_millis(2000));
            continue;
        }

        // Subscribe to changes
        if let Err(e) = write_command_message(
            sock.get_mut(),
            &mut seq,
            protocol::Command::Subscribe(protocol::SubscriptionMask::SINK),
            protocol_version,
        ) {
            error!("Failed to subscribe to PA changes: {e:?}");
            sleep(Duration::from_millis(2000));
            continue;
        }

        // The first reply is just an ACK.
        seq = match protocol::read_ack_message(&mut sock) {
            Err(e) => {
                error!("Failed to read PA ack message: {e:?}");
                sleep(Duration::from_millis(2000));
                continue;
            }
            Ok(s) => s + 1
        };

        info!("Connected pulseaudio main loop, waiting for events...");
        loop {
            let (_, event) = match protocol::read_command_message(&mut sock, protocol_version) {
                Err(e) => {
                    error!("Failed to read PA message: {e:?}");
                    sleep(Duration::from_millis(2000));
                    continue;
                }
                Ok(v) => v
            };

            match event {
                protocol::Command::SubscribeEvent(event) => {
                    trace!("Got subscription event {:?} for ID {:?} ({:?})",
                        event.event_type, event.index, event.event_facility
                    );
                    if let Some(index) = event.index {
                        if let Err(e) = query_volume(
                            &mut sock,
                            &mut seq,
                            protocol_version,
                            protocol::command::GetSinkInfo { index: Some(index), name: None },
                            Some(output_name.clone()),
                            volume_signal_tx.clone(),
                        ) {
                            error!("Failed to update PA volume: {e:?}");
                            sleep(Duration::from_millis(2000));
                            continue;
                        }
                    }
                },
                _ => warn!("Got unexpected pulseaudio event {:?}", event),
            }
        }
    }
}

pub fn pulseaudio_action(
    output_name: &str,
    volume_action_rx: mpsc::Receiver<VolumeCommand>,
    max_volume: u8,
) -> anyhow::Result<()> {
    let output_name = CString::new(output_name)?;
    let mut connection_failed = false;
    loop {
        trace!("Initiating PA action connection...");
        let mut seq: u32 = 0;
        let (mut sock, protocol_version) = match connect_to_pulseaudio(&mut seq, CString::new("mpd-sink-sync-action")?) {
            Err(e) => {
                if !connection_failed {
                    warn!("Failed to connect to pulseaudio: {e:?}");
                    // Don't spam logs too much
                    connection_failed = true;
                }
                sleep(Duration::from_millis(2000));
                continue;
            }
            Ok(v) => v
        };
        connection_failed = false;
        loop {
            trace!("Waiting for PA actions...");
            let action = volume_action_rx.recv();
            match action {
                Ok(VolumeCommand::Set { volume, .. }) => {
                    let mut cvolume = protocol::ChannelVolume::empty();
                    let pa_volume = pa_volume_from_u8(volume.min(max_volume));
                    cvolume.push(pa_volume);
                    debug!("Executing pulseaudio action: {pa_volume}");
                    if let Err(e) = write_command_message(
                        sock.get_mut(),
                        &mut seq,
                        protocol::Command::SetSinkVolume(
                            protocol::command::SetDeviceVolumeParams {
                                device_index: None,
                                device_name: Some(output_name.clone()),
                                volume: cvolume,
                            },
                        ),
                        protocol_version,
                    ) {
                        error!("Failed to execute pulseaudio action: {e:?}");
                        sleep(Duration::from_millis(2000));
                        break;
                    }
                }
                Ok(VolumeCommand::None) => {
                    trace!("Got empty PA action");
                }
                Err(e) => {
                    error!("Failed to retrieve PA action: {e:?}");
                }
            }
        }
    }
}

fn pa_volume_from_u8(volume: u8) -> protocol::Volume {
    let volume_u32 = (protocol::Volume::NORM.as_u32() as f32 * ((volume as f32) / 100.0)).round() as u32;
    trace!("Converted volume: {volume} -> {volume_u32}");
    protocol::Volume::from_u32_clamped(volume_u32)
}

fn query_volume(
    sock: &mut std::io::BufReader<UnixStream>,
    seq: &mut u32,
    protocol_version: u16,
    sink_info: protocol::command::GetSinkInfo,
    output_name: Option<CString>,
    volume_signal_tx: watch::WatchSender<VolumeCommand>,
) -> Result<(), protocol::ProtocolError> {
    write_command_message(
        sock.get_mut(),
        seq,
        protocol::Command::GetSinkInfo(sink_info),
        protocol_version,
    )?;
    let (_, reply): (u32, protocol::command::SinkInfo) = protocol::read_reply_message(sock, protocol_version)?;
    if output_name.map(|name| name == reply.name).unwrap_or(true) {
        if let Some(volume) = reply.cvolume.channels().first() {
            // Volume.to_linear() is actual cubic, so do manually... :/
            let f = volume.as_u32() as f32 / protocol::serde::Volume::NORM.as_u32() as f32;
            // Convert to [0, 100]
            let volume = (f * 100.0).round() as u8;
            info!("Pulseaudio volume updated to {volume}");
            volume_signal_tx.send(VolumeCommand::Set { volume, source: "".to_string() });
        }
    }
    Ok(())
}

fn write_command_message<W: std::io::Write>(w: &mut W, seq: &mut u32, command: protocol::Command, protocol_version: u16) -> Result<(), protocol::ProtocolError> {
    protocol::write_command_message(
        w,
        *seq,
        command,
        protocol_version,
    )?;
    *seq += 1;
    Ok(())
}
