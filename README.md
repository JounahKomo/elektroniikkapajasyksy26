# Radiophone (Elektroniikkapaja - Aalto University)

This repository contains the embedded firmware for a custom radiophone (two-way radio / walkie-talkie), developed as part of the **Elektroniikkapaja** (Electronics Workshop) course at **Aalto University**.

The firmware is written in bare-metal Rust utilizing the asynchronous [Embassy](https://embassy.dev/) framework.

---

## Project Overview

The objective of this project is to design and build a functional radiophone. The device handles audio input and output, digital signal processing/control, and radio-frequency (RF) transmission and reception.

### Hardware

- **Microcontroller**: STMicroelectronics STM32L432KC (ARM Cortex-M4 with FPU)
- **Development Board**: [NUCLEO-L432KC](https://www.st.com/en/evaluation-tools/nucleo-l432kc.html) (Nucleo-32 form factor)
- **Peripherals & RF Modules**: Microphones, audio amplifiers/speakers, PTT (Push-to-Talk) controls, and RF transceiver modules (integrated as development progresses).

### Software Stack & Tools

- **Language**: [Rust](https://www.rust-lang.org/) (`#![no_std]`, bare-metal)
- **Target Architecture**: `thumbv7em-none-eabihf` (ARM Cortex-M4 with hard-float)
- **Async Runtime**: [Embassy](https://github.com/embassy-rs/embassy) (`embassy-stm32`, `embassy-executor`, `embassy-time`)
- **Logging & Diagnostics**: [`defmt`](https://github.com/knurling-rs/defmt) over RTT (`defmt-rtt`)
- **Flashing & Debugging**: [`probe-rs`](https://probe.rs/)
- **Linker & Stack Protection**: [`flip-link`](https://github.com/knurling-rs/flip-link)
- **Development Assistant**: Antigravity (agentic workflows for bringup, scaffolding, and verification)

---

## Development Workflow

This project is developed with **Antigravity**, leveraging agentic AI workflows to assist with embedded bringup, peripheral configuration, and code iteration alongside hands-on bare-metal Rust development.

---

## Project Structure

```
├── .cargo/
│   └── config.toml      # Build target configuration, probe-rs runner, and linker flags
├── src/
│   ├── bin/
│   │   ├── blinky.rs        # Hardware bringup demo (blinks onboard LED LD3 on PB3)
│   │   └── external_led.rs  # Blinks an external LED on D12 (PB4)
│   └── main.rs          # Main radiophone firmware application
├── Cargo.toml           # Project manifest and dependencies
└── README.md            # Project documentation
```

---

## Getting Started

### Prerequisites

1. **Rust Toolchain**: Install Rust via [rustup](https://rustup.rs/):
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. **ARM Cortex-M4 Target**:
   ```bash
   rustup target add thumbv7em-none-eabihf
   ```

3. **Required Cargo Tools**:
   ```bash
   # Stack-overflow protection linker wrapper
   cargo install flip-link

   # Flashing and debugging tool
   cargo install probe-rs-tools
   ```

### Building and Running

Connect the Nucleo-L432KC board via USB.

1. **Run the Blinky Demo**:
   To verify that your toolchain, debug probe, and board connection work:
   ```bash
   cargo run --bin blinky
   ```
   This will flash the microcontroller, start execution, and stream defmt log output to your terminal. The onboard user LED (LD3, connected to `PB3`) will blink at 200 ms intervals.

2. **Run the External LED Demo**:
   Connect an LED with a series resistor (~330 Ω) between `D12` (`PB4`) and GND, then:
   ```bash
   cargo run --bin external_led
   ```
   This will flash and blink the external LED at 500 ms intervals.

3. **Run the Main Firmware**:
   ```bash
   cargo run
   ```
   For optimized release builds:
   ```bash
   cargo run --release
   ```

---

## Roadmap

- [x] Hardware setup and project template with Embassy on STM32L432KC
- [x] Basic onboard LED blinking and defmt logging over RTT
- [x] External LED output demo (GPIO PB4)
- [ ] Push-to-Talk (PTT) button input handling with debouncing
- [ ] Audio capture (microphone ADC / I2S)
- [ ] Audio playback (DAC / PWM / I2S audio amplifier)
- [ ] RF transceiver interface (SPI / I2C module integration)
- [ ] Half-duplex transmit and receive state machine
- [ ] Enclosure and final hardware assembly

---

## License

This project is created for educational purposes in the Elektroniikkapaja course at Aalto University.
