#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, OutputType, Speed};
use embassy_stm32::time::Hertz;
use embassy_stm32::timer::low_level::CountingMode;
use embassy_stm32::timer::simple_pwm::{PwmPin, SimplePwm};
use embassy_time::{Duration, Timer};
use {defmt_rtt as _, panic_probe as _};

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_stm32::init(Default::default());

    // Onboard green LED LD3 on PB3 (blinks in sync with beep)
    let mut led = Output::new(p.PB3, Level::Low, Speed::Low);

    Timer::after(Duration::from_millis(500)).await;
    info!("===============================================");
    info!("=== Grove Speaker v1.1 PWM Beep Test        ===");
    info!("===============================================");
    info!("Signal pin: PA8 (TIM1_CH1, Nucleo D9)");
    info!("Power: VCC -> 5V, GND -> GND");

    // Configure PA8 as TIM1 CH1 PWM output
    let ch1 = PwmPin::new_ch1(p.PA8, OutputType::PushPull);
    let mut pwm = SimplePwm::new(
        p.TIM1,
        Some(ch1),
        None,
        None,
        None,
        Hertz(1000), // 1 kHz beep tone
        CountingMode::EdgeAlignedUp,
    );

    let mut pwm_ch1 = pwm.ch1();
    pwm_ch1.enable();

    let mut beep_count: u32 = 0;

    loop {
        beep_count += 1;
        info!("Playing beep #{} (1000 Hz)...", beep_count);

        // Turn on 1 kHz beep with 50% duty cycle square wave
        led.set_high();
        let max_duty = pwm_ch1.max_duty_cycle();
        pwm_ch1.set_duty_cycle(max_duty / 2);

        Timer::after(Duration::from_millis(300)).await;

        // Turn off sound (0% duty cycle)
        led.set_low();
        pwm_ch1.set_duty_cycle(0);

        Timer::after(Duration::from_millis(1000)).await;
    }
}
