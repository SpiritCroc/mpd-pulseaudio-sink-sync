use crate::command::VolumeCommand;

use log::{trace, debug, warn, error};

use std::sync::mpsc;
use std::thread::sleep;
use std::time::Duration;

use mpd::Client;
use mpd::Idle;

pub fn listen_mpd_client(
    address: &str,
    volume_action_tx: mpsc::Sender<VolumeCommand>,
    mut volume_signal_rx: watch::WatchReceiver<VolumeCommand>,
) {
    let mut connection_failed = false;
    loop {
        let mut conn = match Client::connect(address) {
            Err(e) => {
                if !connection_failed {
                    warn!("L/{address}: Failed to connect: {e:?}");
                     // Don't spam logs too much
                    connection_failed = true;
                }
                sleep(Duration::from_millis(2000));
                continue;
            }
            Ok(c) => c
        };
        let mut last_volume = match conn.status() {
            Err(e) => {
                warn!("L/{address} Failed to read status: {e:?}");
                continue;
            }
            Ok(s) => s.volume
        };
        connection_failed = false;
        debug!("L/{address}: Initial volume: {last_volume}");
        // Idle loop
        loop {
            match conn.idle(&[mpd::Subsystem::Mixer]) {
                Err(e) => {
                    warn!("L/{address}: Failed to start idling: {e:?}");
                    break;
                }
                Ok(guard) => {
                    trace!("L/{address}: Idling...");
                    if let Err(e) = guard.get() {
                        warn!("{address}: Failed to idle: {e:?}");
                        break;
                    }
                }
            };
            let volume = match conn.status() {
                Err(e) => {
                    warn!("L/{address}: Failed to read status: {e:?}");
                    break;
                }
                Ok(s) => s.volume
            };
            if volume != last_volume {
                let action_volume = match volume.try_into() {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("L/{address}: Invalid volume reported: {volume}, {e:?}");
                        continue;
                    }
                };
                match volume_signal_rx.get() {
                    VolumeCommand::Set { volume, .. } if volume == action_volume => {
                        debug!("L/{address}: Acked volume: {volume}");
                    }
                    _ => {
                        debug!("L/{address}: Updated volume: {volume}");
                        last_volume = volume;
                        let action = VolumeCommand::Set {
                            volume: action_volume,
                            source: address.to_string(),
                        };
                        if let Err(e) = volume_action_tx.send(action) {
                                error!("L/{address}: Failed to initiate volume change, {e:?}");
                        }
                    }
                }
            }
        }
    }
}

fn handle_volume_signal_in_mpd(address: &str, conn: &mut Client, action: &VolumeCommand) -> anyhow::Result<()> {
    match action {
        VolumeCommand::None => Ok(()),
        VolumeCommand::Set { volume, source } => {
            if source == address {
                trace!("S/{address}: ignore volume change by self");
                Ok(())
            } else {
                let volume = match (*volume).try_into() {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("S/{address}: Invalid volume to set: {volume}, {e:?}");
                        return Ok(());
                    }
                };
                debug!("S/{address}: Applying volume signal: {volume}");
                conn.volume(volume)?;
                Ok(())
            }
        }
    }
}

pub fn reflect_volume_to_mpd(address: &str, mut volume_signal_rx: watch::WatchReceiver<VolumeCommand>) -> anyhow::Result<()> {
    let mut connection_failed = false;
    loop {
        let mut conn = match Client::connect(address) {
            Err(e) => {
                if !connection_failed {
                    warn!("S/{address}: Failed to connect: {e:?}");
                    // Don't spam logs too much
                    connection_failed = true;
                }
                sleep(Duration::from_millis(2000));
                continue;
            }
            Ok(c) => c
        };
        connection_failed = false;
        let initial_signal = volume_signal_rx.get();
        if let Err(e) = handle_volume_signal_in_mpd(address, &mut conn, &initial_signal) {
            error!("S/{address}: Failed to execute action: {e:?}");
        }
        loop {
            let signal = volume_signal_rx.wait();
            if let Err(e) = handle_volume_signal_in_mpd(address, &mut conn, &signal) {
                error!("S/{address}: Failed to execute action: {e:?}");
                sleep(Duration::from_millis(2000));
                break;
            }
        }
    }
}
