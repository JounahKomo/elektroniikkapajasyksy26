# Audio Assets for Radiophone

This directory contains raw audio files formatted for playback on the STM32 microcontroller via PWM on the Grove Speaker v1.1.

## Format Specifications
- **Format**: Raw 8-bit unsigned PCM (`u8`, value range 0 to 255 with 128 as the DC midpoint)
- **Channels**: 1 (Mono)
- **Sample Rate**: 8,000 Hz (8 kHz)
- **Storage Cost**: ~8 KB per second of audio

## Available Files

| File | Duration | Size | Source / Description |
| :--- | :--- | :--- | :--- |
| `chime.raw` | 2.26 s | 18,041 B | Melodious chime ([Wikimedia Commons](https://commons.wikimedia.org/wiki/File:Chime.wav), CC BY-SA) |
| `bell.raw` | 1.80 s | 14,400 B | Bicycle bell ring ([Wikimedia Commons](https://commons.wikimedia.org/wiki/File:Bike-bell.wav), CC0) |
| `roger_beep.raw` | 0.28 s | 2,240 B | Dual-tone walkie-talkie roger beep (1000 Hz + 1500 Hz) |

## How to Select an Audio File
In `src/bin/audio_player.rs`, configure the `AUDIO_TRACK` constant:
```rust
// Select which audio track to play:
const SELECTED_TRACK: AudioTrack = AudioTrack {
    name: "Chime",
    data: include_bytes!("../../audio/chime.raw"),
};
// Or select:
// const SELECTED_TRACK: AudioTrack = AudioTrack { name: "Bicycle Bell", data: include_bytes!("../../audio/bell.raw") };
// const SELECTED_TRACK: AudioTrack = AudioTrack { name: "Roger Beep", data: include_bytes!("../../audio/roger_beep.raw") };
```

## Adding New Audio Files
To convert any standard WAV file to 8 kHz unsigned 8-bit raw PCM:
```bash
python3 audio/convert.py my_sound.wav audio/my_sound.raw audio/my_sound_8k.wav
```
