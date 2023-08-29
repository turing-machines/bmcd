# bmcd

`bmcd` or 'BMC Daemon' is part of the
[BMC-Firmware](https://www.github.com/turing-machines/BMC-Firmware) and is responsible
for hosting Restful APIs related to node management, and configuration of a Turing-Pi 2 board.

## Building

This package will be built as part of the buildroot firmware located
[here](https://www.github.com/turing-machines/BMC-Firmware). If you want to
build the the binary from this repository, we recommend to use `cargo cross`:

```bash
# install cross environment
cargo install cross --git https://github.com/cross-rs/cross

# execute cross build command for Turing-Pi target
cross build  --target armv7-unknown-linux-gnueabi -p bmcd
```

