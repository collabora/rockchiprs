# Rockchip usb protocol host implementation

Rockchip bootroms and early loaders implement an USB protocol to help loader
early firmware, flashing persistant storage etc. This crate contains a sans-io
implementation of that protocol as well as an optional (enabled by default)
implementation of IO using libusb

```rust,no_run
fn main() -> anyhow::Result<()> {
    let devices = rockusb::libusb::Devices::new()?;
    let mut transport = devices.iter().next()
    	.ok_or_else(|| anyhow::anyhow!("No Device found"))??;
    println!("Chip Info: {:0x?}", transport.chip_info()?);
    Ok(())
}
```
