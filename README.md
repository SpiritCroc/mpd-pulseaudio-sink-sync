# mpd-pulseaudio-sync-sink

A daemon that listens to MPD volume changes from possibly multiple MPD servers and applies them to a single pulseaudio (/pipewire) sink.  
You probably want to set your MPD's `mixer_type` to `null` to disable internal volume handling but still allow controlling volume. [^1]

[^1]: https://mpd.readthedocs.io/en/latest/user.html#external-mixer

#### Use when

- You want the MPD volume to only apply to a specific pulseaudio sink.
- You have one or multiple MPD servers, or other music players speaking the MPD protocol, sharing the same audio output

#### Do not use when

- You only have one audio device and one MPD anyway, in which case you can use MPD's inbuilt volume control
- All you want is breakfast
