# Rockchip usb protocol host implementation

Rockchip bootroms and early loaders implement an USB protocol to help loader
early firmware, flashing persistant storage etc. This crate contains a sans-io
implementation of that protocol as well as an optional (enabled by default)
implementation of IO using libusb


