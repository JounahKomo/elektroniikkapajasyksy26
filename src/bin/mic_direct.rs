#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use nucleo_l432kc_app::max98357a::{config_80mhz, Max98357a};
use {defmt_rtt as _, panic_probe as _};

/// Digital amplification factor for live monitoring.
/// INMP441 sensitivity is -26 dBFS at 94 dB SPL.
/// Normal speech at 20 cm produces ~30..150 peak in top 16 bits.
/// 16x boost scales normal speech to ~500..3,000 peak (clean, clear speaker output).
const GAIN: i32 = 16;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // 80 MHz SYSCLK via PLL for precise cycle-level timing
    let p = embassy_stm32::init(config_80mhz());

    // Onboard green LED LD3 on PB3 (lights up continuously during live pass-through)
    let mut led = Output::new(p.PB3, Level::High, Speed::Low);

    info!("=======================================================");
    info!("=== INMP441 -> MAX98357A Live Direct Audio Feed     ===");
    info!("=======================================================");
    info!("Live pass-through active with ~62.5 µs latency (1 frame).");
    info!("Digital gain: {}x", GAIN);
    info!("");
    info!("Pinout (STM32L432KC Nucleo-32):");
    info!("  INMP441 SCK  -> PA3 [Nucleo A2] (shared with MAX98357A BCLK)");
    info!("  INMP441 WS   -> PA4 [Nucleo A3] (shared with MAX98357A LRC)");
    info!("  INMP441 SD   -> PA1 [Nucleo A1] (digital input to STM32)");
    info!("  INMP441 L/R  -> GND (Left channel) or 3.3V (Right channel)");
    info!("  INMP441 VDD  -> 3.3V, GND -> GND");
    info!("  MAX98357A DIN -> PA10 [Nucleo D0 / RX]");
    info!("  MAX98357A VIN -> 5V (or 3.3V), GND -> GND");
    info!("  MAX98357A '+' / '-' -> Speaker terminals");
    info!("=======================================================");

    // Initialize audio driver with speaker output (PA10) and microphone input (PA1)
    let mut audio = Max98357a::new_with_mic(p.PA3, p.PA4, p.PA10, p.PA1);

    // Startup beep: confirm the amplifier is active and speaker produces sound
    info!("Testing speaker with 150 ms tone...");
    audio.play_tone(1000, 150);
    info!("Speaker test complete. Late bits: {}", audio.late_bits());

    // Keep clocks running for 350 ms so the INMP441 finishes its 2^18-cycle startup filter
    // and automatically detect whether L/R is tied to GND (Left) or VDD (Right).
    info!("Warming up microphone filter and detecting active channel (350 ms)...");
    let mut left_warmup = 0u32;
    let mut right_warmup = 0u32;
    let warmup_frames = (16_000 * 350) / 1000;
    for _ in 0..warmup_frames {
        let (rx_l, rx_r) = audio.exchange_frame(0, 0);
        if rx_l != 0 {
            left_warmup += 1;
        }
        if rx_r != 0 {
            right_warmup += 1;
        }
    }

    let use_right = right_warmup > left_warmup;
    info!(
        "Microphone ready! Selected channel: {} (activity: Left={}, Right={})",
        if use_right { "Right" } else { "Left" },
        left_warmup,
        right_warmup
    );

    info!(">>> LIVE AUDIO FEED ACTIVE: Speak into microphone! <<<");
    led.set_high();

    // DC-blocking high-pass filter state:
    // y[n] = x[n] - x[n-1] + 0.99 * y[n-1]
    let mut prev_x: i32 = 0;
    let mut prev_y: i32 = 0;

    let mut current_tx_sample: u16 = 0;

    // Diagnostic tracking (updated once every 16,000 frames = 1.0 second)
    let mut frame_count: u32 = 0;
    let mut max_raw: i16 = 0;
    let mut max_abs: i16 = 0;

    loop {
        // Exchange one 64-BCLK frame:
        // Transmit current_tx_sample to MAX98357A on DIN (PA10)
        // Receive microphone sample from INMP441 on SD (PA1)
        let (rx_l, rx_r) = audio.exchange_frame(current_tx_sample, current_tx_sample);

        // Read consistently from the selected channel
        let raw_in = if use_right { rx_r } else { rx_l };

        let abs_raw = raw_in.saturating_abs();
        if abs_raw > max_raw {
            max_raw = abs_raw;
        }

        // 1-pole high-pass DC-blocking filter (cutoff ~25 Hz at 16 kHz sample rate)
        let x = raw_in as i32;
        let y = x - prev_x + ((prev_y * 253) >> 8);
        prev_x = x;
        prev_y = y;

        // Apply gain with saturation
        let amplified = (y * GAIN).clamp(i16::MIN as i32, i16::MAX as i32) as i16;

        // Prepare sample to be transmitted to the speaker in the very next frame (62.5 µs latency)
        current_tx_sample = amplified as u16;

        // Track peak amplitude of amplified signal
        let abs_val = amplified.saturating_abs();
        if abs_val > max_abs {
            max_abs = abs_val;
        }

        frame_count += 1;
        if frame_count >= 16_000 {
            info!(
                "Live feed: Raw peak = {} / 32767, Out peak = {} ({}%), Late bits = {}",
                max_raw,
                max_abs,
                (max_abs as u32 * 100) / 32767,
                audio.late_bits()
            );

            // Reset diagnostics for next 1-second window
            frame_count = 0;
            max_raw = 0;
            max_abs = 0;
        }
    }
}
