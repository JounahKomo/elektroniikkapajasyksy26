#!/usr/bin/env python3
"""
Convert an audio file (WAV, MP3, etc.) to 8 kHz, 8-bit unsigned PCM (.raw and .wav)
for embedded PWM audio playback on STM32.
"""

import sys
import wave

def convert_to_pcm8(input_wav, output_raw, output_wav=None, target_rate=8000, start_sec=0.0, max_duration=None):
    with wave.open(input_wav, "rb") as w:
        n_channels = w.getnchannels()
        sampwidth = w.getsampwidth()
        framerate = w.getframerate()
        n_frames = w.getnframes()
        raw_bytes = w.readframes(n_frames)

    step = sampwidth
    decoded = []
    max_val = float(1 << (8 * sampwidth - 1))

    # Decode and average channels to mono
    for i in range(0, len(raw_bytes), step * n_channels):
        val = 0.0
        for ch in range(n_channels):
            offset = i + ch * step
            chunk = raw_bytes[offset:offset+step]
            s = int.from_bytes(chunk, byteorder="little", signed=True)
            val += s / max_val
        val /= n_channels
        decoded.append(val)

    # Crop
    start_idx = int(start_sec * framerate)
    if max_duration:
        end_idx = min(len(decoded), start_idx + int(max_duration * framerate))
    else:
        end_idx = len(decoded)
    decoded = decoded[start_idx:end_idx]

    # Resample to target_rate using linear interpolation
    duration = len(decoded) / framerate
    target_len = int(duration * target_rate)
    resampled = []
    for out_i in range(target_len):
        in_pos = out_i * framerate / target_rate
        idx0 = int(in_pos)
        idx1 = min(idx0 + 1, len(decoded) - 1)
        frac = in_pos - idx0
        val = (1.0 - frac) * decoded[idx0] + frac * decoded[idx1]
        resampled.append(val)

    # Normalize to 95% full scale and map to unsigned 8-bit [0, 255]
    peak = max(abs(x) for x in resampled) or 1.0
    scale = 0.95 / peak
    pcm8 = bytearray(len(resampled))
    for i, x in enumerate(resampled):
        v = int((x * scale * 127.0) + 128.0)
        pcm8[i] = max(0, min(255, v))

    with open(output_raw, "wb") as f:
        f.write(pcm8)

    if output_wav:
        with wave.open(output_wav, "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(1)
            w.setframerate(target_rate)
            w.writeframes(pcm8)

    print(f"Successfully converted '{input_wav}' -> '{output_raw}' ({len(pcm8)} bytes, {len(pcm8)/target_rate:.2f}s @ {target_rate} Hz)")

if __name__ == "__main__":
    if len(sys.argv) < 3:
        print("Usage: python3 convert.py <input.wav> <output.raw> [output.wav] [target_rate=8000]")
        sys.exit(1)
    in_file = sys.argv[1]
    out_raw = sys.argv[2]
    out_wav = sys.argv[3] if len(sys.argv) > 3 else None
    rate = int(sys.argv[4]) if len(sys.argv) > 4 else 8000
    convert_to_pcm8(in_file, out_raw, out_wav, target_rate=rate)
