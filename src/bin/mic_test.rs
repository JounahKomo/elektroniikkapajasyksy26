#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use defmt::info;
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use nucleo_l432kc_app::max98357a::{config_80mhz, Max98357a};
use {defmt_rtt as _, panic_probe as _};

/// Recording sample rate (8,000 Hz)
const SAMPLE_RATE: u32 = 8_000;

/// Recording duration in seconds (2.0 seconds)
/// 2.0 seconds @ 8 kHz = 16,000 samples = 32,000 bytes (fits comfortably in 64 KB RAM)
const RECORD_SECONDS: u32 = 2;
const BUFFER_SAMPLES: usize = (SAMPLE_RATE * RECORD_SECONDS) as usize;

struct SafeBuffer(UnsafeCell<[i16; BUFFER_SAMPLES]>);
unsafe impl Sync for SafeBuffer {}

static AUDIO_BUFFER: SafeBuffer = SafeBuffer(UnsafeCell::new([0; BUFFER_SAMPLES]));

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // 80 MHz SYSCLK via PLL for precise cycle-level timing
    let p = embassy_stm32::init(config_80mhz());

    // Onboard green LED LD3 on PB3 (lights up during recording and playback)
    let mut led = Output::new(p.PB3, Level::Low, Speed::Low);

    info!("=======================================================");
    info!("=== Walkie-Talkie Microphone & Speaker Echo Test    ===");
    info!("=======================================================");
    info!("Audio format: 16-bit PCM @ 8,000 Hz");
    info!(
        "Buffer length: {} samples (~{} seconds, {} bytes RAM)",
        BUFFER_SAMPLES,
        RECORD_SECONDS,
        BUFFER_SAMPLES * 2
    );
    info!("");
    info!("--- INMP441 MEMS Microphone Pinout ---");
    info!("  VDD     -> 3.3V (Nucleo 3V3)");
    info!("  GND     -> GND");
    info!("  L/R     -> GND (Left channel) or 3.3V (Right channel)");
    info!("  SCK     -> PA3 [Nucleo A2, CN4 pin 10] (shared with MAX98357A BCLK)");
    info!("  WS      -> PA4 [Nucleo A3, CN4 pin 9]  (shared with MAX98357A LRC)");
    info!("  SD      -> PA1 [Nucleo A1, CN4 pin 11] (digital audio input to STM32)");
    info!("");
    info!("--- MAX98357A I2S Amplifier Pinout ---");
    info!("  VIN     -> 5V (or 3.3V)");
    info!("  GND     -> GND");
    info!("  BCLK    -> PA3 [Nucleo A2] (shared with INMP441 SCK)");
    info!("  LRC     -> PA4 [Nucleo A3] (shared with INMP441 WS)");
    info!("  DIN     -> PA10 [Nucleo D0 / RX, CN3 pin 2]");
    info!("  SD_MODE -> Leave unconnected or tie to 3.3V");
    info!("  GAIN    -> Leave unconnected (9 dB standard gain)");
    info!("  '+' / '-' -> Speaker terminals");
    info!("=======================================================");

    // Initialize audio driver with both speaker output (PA10) and microphone input (PA1)
    let mut audio = Max98357a::new_with_mic(p.PA3, p.PA4, p.PA10, p.PA1);

    // Warm-up check: confirm the timing runs with 0 late bits
    audio.play_tone(1000, 200);
    info!("Audio driver initialized. Late bits: {}", audio.late_bits());

    // Keep clocks running for 350 ms so the INMP441 finishes its 2^18-cycle startup filter
    info!("Warming up microphone filter (350 ms continuous clocks)...");
    audio.idle_clocks(350);

    let mut loop_count: u32 = 0;

    loop {
        loop_count += 1;
        info!("");
        info!("--- Loop #{} ---", loop_count);

        // 1. Play two short high beeps to signal "START SPEAKING"
        info!("Ready... Speak into microphone when LED lights up!");
        audio.play_tone(1500, 60);
        audio.idle_clocks(60);
        audio.play_tone(1500, 80);
        audio.idle_clocks(150);

        // 2. Record 2.0 seconds from INMP441 microphone
        info!(
            ">>> RECORDING ({} seconds) - speak now! <<<",
            RECORD_SECONDS
        );
        led.set_high();

        let buf = unsafe { &mut *AUDIO_BUFFER.0.get() };

        // Test channel activity and record
        let (channel, active_count) = audio.record_pcm_i16(buf, SAMPLE_RATE);
        let use_right = channel == "Right";

        // If right channel was the active one, record designated channel directly
        if use_right {
            audio.record_channel(buf, SAMPLE_RATE, true);
        }

        led.set_low();
        info!(
            "Recording finished! Active channel: {}, non-zero samples: {} / {}",
            channel, active_count, BUFFER_SAMPLES
        );

        // 3. Audio processing:
        //    a) Calculate and subtract DC bias (mean)
        //    b) Measure peak amplitude
        //    c) Apply 4x digital gain with saturation
        let mut sum: i64 = 0;
        for &s in buf.iter() {
            sum += s as i64;
        }
        let dc_offset = (sum / buf.len() as i64) as i16;

        let mut max_abs: i16 = 0;
        for s in buf.iter_mut() {
            // Remove DC bias
            let centered = s.saturating_sub(dc_offset);
            // Measure peak
            let abs_val = centered.saturating_abs();
            if abs_val > max_abs {
                max_abs = abs_val;
            }
            // 4x digital gain for loud, clear playback on the speaker
            *s = centered.saturating_mul(4);
        }

        info!(
            "Raw samples [0..4]: [{}, {}, {}, {}]",
            buf[0], buf[1], buf[2], buf[3]
        );
        info!(
            "Signal analysis: DC offset = {}, Peak amplitude = {} / 32767",
            dc_offset, max_abs
        );

        if max_abs < 100 {
            info!("WARNING: No audio signal detected on PA1. Check INMP441 wiring (VDD=3.3V, GND, SCK=PA3, WS=PA4, SD=PA1)!");
        } else {
            info!(
                "Voice detected! Volume level: {}%",
                (max_abs as u32 * 100) / 32767
            );
        }

        // 4. Brief pause before playback (keeping clocks alive)
        audio.idle_clocks(300);

        // 5. Play back the recorded audio through the speaker
        info!(">>> PLAYING BACK recorded audio on speaker... <<<");
        led.set_high();
        audio.play_pcm_i16(buf, SAMPLE_RATE);
        led.set_low();
        info!("Playback completed.");

        // 6. Pause 1.5 seconds before starting the next listen-and-repeat cycle
        audio.idle_clocks(1500);
    }
}
