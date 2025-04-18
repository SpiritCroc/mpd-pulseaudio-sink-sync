#[derive(Debug, Clone)]
pub enum VolumeCommand {
    None,
    Set {
        volume: u8,
        source: String,
    }
}
