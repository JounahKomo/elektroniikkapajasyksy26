# Radiophone (Elektroniikkapaja - Aalto University)

This repository contains the embedded firmware for a custom radiophone (two-way radio / walkie-talkie), developed as part of the **Elektroniikkapaja** (Electronics Workshop) course at **Aalto University**.

The firmware is written in bare-metal Rust utilizing the asynchronous [Embassy](https://embassy.dev/) framework.

---

## Project Overview

The objective of this project is to design and build a functional radiophone. The device handles audio input and output, digital signal processing/control, and radio-frequency (RF) transmission and reception.

### Hardware

- **Microcontroller**: STMicroelectronics STM32L432KC (ARM Cortex-M4 with FPU)
- **Development Board**: [NUCLEO-L432KC](https://www.st.com/en/evaluation-tools/nucleo-l432kc.html) (Nucleo-32 form factor)
- **Peripherals & RF Modules**:
  - NRF24L01 2.4 GHz RF transceiver module
  - Grove Speaker v1.1 active speaker module (LM386 amplifier)
  - Microphones, audio amplifiers, and PTT controls (integrated as development progresses)

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
├── audio/               # Audio assets (8 kHz, 8-bit unsigned PCM) and conversion tool
│   ├── README.md        # Audio library documentation & selection guide
│   ├── chime.raw        # Chime sound effect (Wikimedia Commons)
│   ├── bell.raw         # Bicycle bell ring (Wikimedia Commons)
│   ├── roger_beep.raw   # Walkie-talkie dual-tone roger beep
│   └── convert.py       # Helper script to convert standard WAV files to PCM
├── src/
│   ├── bin/
│   │   ├── blinky.rs        # Hardware bringup demo (blinks onboard LED LD3 on PB3)
│   │   ├── external_led.rs  # Blinks an external LED on D12 (PB4)
│   │   ├── nrf24_test.rs    # NRF24L01 SPI communication and register verification test
│   │   ├── nrf24_tx.rs      # NRF24L01 packet transmitter test (Enhanced ShockBurst TX)
│   │   ├── nrf24_rx.rs      # NRF24L01 packet receiver test (Enhanced ShockBurst RX)
│   │   ├── speakers.rs      # Grove Speaker v1.1 PWM beep test (TIM1_CH1 on PA8)
│   │   └── audio_player.rs  # Audio file player via high-frequency PWM on Grove Speaker
│   ├── lib.rs           # Library root exporting reusable modules
│   ├── nrf24.rs         # NRF24L01 driver module (registers, TX/RX configuration, packet handling)
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
   This will flash the microcontroller, start execution, and stream defmt log output to your terminal. The onboard user LED (LD3, connected to `PB3`) will blink at 500 ms intervals.

2. **Run the External LED Demo**:
   Connect an LED with a series resistor (~330 Ω) between `D12` (`PB4`) and GND, then:
   ```bash
   cargo run --bin external_led
   ```
   This will flash and blink the external LED at 500 ms intervals.

3. **Run the NRF24L01 SPI Test**:
   Wiring connections:
   - **VCC** -> 3.3V
   - **GND** -> GND
   - **CSN** -> PA11 (D10)
   - **CE** -> PB0 (D3)
   - **SCK** -> PA5 (A4)
   - **MOSI** -> PA7 (A6)
   - **MISO** -> PA6 (A5)

   ```bash
   cargo run --bin nrf24_test
   ```
   This binary reads internal registers (`CONFIG`, `STATUS`, `RF_CH`, `TX_ADDR`) and performs a register write/read-back verification to validate SPI communication over RTT defmt logs.

4. **Run the NRF24L01 Transmitter (TX)**:
   ```bash
   cargo run --bin nrf24_tx
   ```
   Configures the module on Channel 76 (2476 MHz) with 32-byte payloads and sends periodic ping packets. Supports Auto-Acknowledgment (Enhanced ShockBurst) and logs ACK receipt confirmation over RTT.

5. **Run the NRF24L01 Receiver (RX)**:
   ```bash
   cargo run --bin nrf24_rx
   ```
   Listens on Channel 76 (2476 MHz) for 32-byte packets, automatically sends hardware ACKs, logs payload text and raw hex bytes over RTT, and pulses the onboard LED on receipt.

6. **Run the Grove Speaker Beep Test**:
   Wiring connections for Grove Speaker v1.1:
   - **VCC (Red)** -> +5V (Nucleo 5V supply, 4.0V–5.5V operating range)
   - **GND (Black)** -> GND
   - **NC** -> Not Connected
   - **SIG (White/Yellow)** -> PA8 (D9, TIM1_CH1 PWM output)

   ```bash
   cargo run --bin speakers
   ```
   Generates a 1000 Hz square wave beep tone (300 ms on, 1000 ms off) via hardware timer PWM, with the onboard LED LD3 flashing in sync.

7. **Run the Audio File Player**:
   ```bash
   cargo run --bin audio_player
   ```
   Plays real 8 kHz 8-bit PCM audio clips over the Grove Speaker using 64 kHz carrier PWM on `PA8`. You can select which track to play (e.g. `chime`, `bell`, or `roger_beep`) or enable cycling through all tracks in [`src/bin/audio_player.rs`](file:///home/joona/elepaja/src/bin/audio_player.rs). See [`audio/README.md`](file:///home/joona/elepaja/audio/README.md) for details on adding custom audio files.

8. **Run the Main Firmware**:
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
- [x] NRF24L01 SPI communication test and register verification
- [x] NRF24L01 packet TX/RX driver and test programs (`nrf24_tx`, `nrf24_rx`)
- [x] Audio playback test (PWM tone generator & audio waveform player on PA8 / TIM1_CH1)
- [ ] Push-to-Talk (PTT) button input handling with debouncing
- [ ] Audio capture (microphone ADC / I2S)
- [ ] Half-duplex transmit and receive state machine
- [ ] Enclosure and final hardware assembly

---

## License

This project is created for educational purposes in the Elektroniikkapaja course at Aalto University.
