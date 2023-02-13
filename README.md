# Utility crates to interact with Rockchip devices

Rockchip SoCs implement a custom USB protocol when starting in a special
recovery mode (sometimes called maskrom). This repository contains helper
crates and examples for interacting with this protocol and the typical files
distributed for early flashing.

* [rockusb](rockusb/README.md) - A crate implementing the client side of the rockchip usb protocol
* [rockfile](rockfile/README.md) - A crate implementing helpers for rockchip specific file formats
