# Linuxstats

Linuxstats is a lightweight desktop system monitor for Linux. It shows live CPU, memory, and disk usage, the top processes, and fan speed readings when the system exposes them.

## Features

- Real-time CPU, memory, and disk usage
- Top processes sorted by CPU usage
- Fan RPM monitoring for Linux and Apple SMC systems
- High-fan-speed warning with process highlighting
- First-run tutorial for new users
- Small, simple desktop UI built with egui

## Screenshots

Add a screenshot here later:

```text
[Your screenshot goes here]
```

## Requirements

- Linux
- Rust toolchain for building from source
- Debian/Ubuntu package build dependencies (when building from source)

## Installation

### Install from GitHub Releases

1. Go to the Releases page:
   https://github.com/aayanmasud/linuxstats/releases
2. Download the latest `.deb` package.
3. Install it:

```bash
sudo apt install ./linuxstats_*.deb
```

If your desktop environment supports it, you can also double-click the package to install it through the system package installer.

### Install from source

```bash
git clone https://github.com/aayanmasud/linuxstats.git
cd linuxstats
cargo run
```

For a release build:

```bash
cargo build --release
./target/release/linuxstats
```

## Running the app

After installation, launch it from your app menu or run:

```bash
linuxstats
```

## Development

Build the Debian package:

```bash
cargo deb
```

## Notes

- Fan speed only appears when your Linux system exposes it through `/sys/class/hwmon`, `/sys/devices/platform`, or Apple SMC sensor paths.
- Warning messages are advisory and intended to help identify possible heat-related issues, not to confirm the root cause.

## License

This project is distributed under the terms of the MIT license unless otherwise specified.
