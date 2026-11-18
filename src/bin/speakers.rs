#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use nucleo_l432kc_app::max98357a::{config_80mhz, Max98357a};
use {defmt_rtt as _, panic_probe as _};

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // 80 MHz SYSCLK via PLL for precise cycle-level timing
    let p = embassy_stm32::init(config_80mhz());

    // Onboard green LED LD3 (lit while the tone plays)
    let mut led = Output::new(p.PB3, Level::High, Speed::Low);

    info!("=======================================================");
    info!("=== MAX98357A Continuous I2S Tone Test (1 kHz)      ===");
    info!("=======================================================");
    info!("I2S Pinout (STM32L432KC Nucleo-32):");
    info!("  BCLK    -> PA3  [Arduino header pin A2, CN4 pin 10]");
    info!("  LRC/WS  -> PA4  [Arduino header pin A3, CN4 pin 9]");
    info!("  DIN     -> PA10 [Arduino header pin D0 / RX, CN3 pin 2]");
    info!("Power & Config:");
    info!("  VIN     -> 5V   (or 3.3V)");
    info!("  GND     -> GND");
    info!("  SD_MODE -> Unconnected (or 3.3V)");
    info!("  GAIN    -> Unconnected (9 dB)");
    info!("Passive Speaker: MAX98357A '+' and '-' -> speaker terminals");

    let mut amp = Max98357a::new(p.PA3, p.PA4, p.PA10);

    // Timing self-check: play 500 ms and report how many bits were clocked out late.
    amp.play_tone(1000, 500);
    info!(
        "Timing check: {} late bits in 500 ms (must be 0)",
        amp.late_bits()
    );

    info!("Playing continuous 1 kHz sine, clocks never stop...");
    loop {
        led.set_high();
        amp.play_tone(1000, 60_000);
    }
}
