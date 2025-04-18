mod command;
mod mpdctrl;
mod pulsectrl;

use command::VolumeCommand;
use pulsectrl::{pulseaudio_listen, pulseaudio_action};
use mpdctrl::{listen_mpd_client, reflect_volume_to_mpd};

use std::thread;
use std::sync::mpsc;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, required = true)]
    output: String,
    #[arg(short, required = true)]
    mpd_addresses: Vec<String>,
    #[arg(short, long, default_value_t = 100)]
    volume_max: u8,
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug")).init();

    let args = Args::parse();

    // Actions MPD -> PA
    let (volume_action_tx, volume_action_rx) = mpsc::channel();
    // Signals of actual volume PA -> MPD
    let (volume_signal_tx, volume_signal_rx) = watch::channel(VolumeCommand::None);

    for address in args.mpd_addresses {
        let vat = volume_action_tx.clone();
        let vsr = volume_signal_rx.clone();
        let addr = address.clone();
        thread::spawn(move || { listen_mpd_client(&addr, vat, vsr) });
        let vsr = volume_signal_rx.clone();
        thread::spawn(move || { reflect_volume_to_mpd(&address, vsr) });
    }

    let output = args.output.clone();
    thread::spawn(move || { pulseaudio_action(&output, volume_action_rx, args.volume_max) });
    pulseaudio_listen(&args.output, volume_signal_tx.clone());

    Ok(())
}
