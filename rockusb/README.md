# Rockchip usb protocol host implementation

Rockchip bootroms and early loaders implement an USB protocol to help loader
early firmware, flashing persistant storage etc. This crate contains a sans-io
implementation of that protocol as well as an optional implementations of IO
using libusb or nusb.

Printing chip info using libusb backend:
```rust,no_run
# #[cfg(feature = "libusb")] {
# fn main() -> anyhow::Result<()> {
let devices = rockusb::libusb::Devices::new()?;
let mut device = devices.iter().next()
    .ok_or_else(|| anyhow::anyhow!("No Device found"))??;
println!("Chip Info: {:0x?}", device.chip_info()?);
Ok(())
# }
# }
```

Printing chip info using nusb backend:
```rust,no_run
# #[cfg(feature = "nusb")] {
# #[tokio::main]
# async fn main() -> anyhow::Result<()> {
let mut devices = rockusb::nusb::devices()?;
let info = devices.next()
    .ok_or_else(|| anyhow::anyhow!("No Device found"))?;
let mut device = rockusb::nusb::Device::from_usb_device_info(info)?;
println!("Chip Info: {:0x?}", device.chip_info().await?);
Ok(())
# }
# }
```
