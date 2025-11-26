mod common;

use anyhow::{Result, anyhow};
use clap::Parser;
use common::{Command, ExampleDeviceAsync, Opts};
use rockusb::nusb::Device;

fn list_available_devices() -> Result<()> {
    let devices = rockusb::nusb::devices()?;
    println!("Available rockchip devices:");
    for d in devices {
        println!(
            "* Bus {} Device {} ID {}:{}",
            d.busnum(),
            d.device_address(),
            d.vendor_id(),
            d.product_id()
        );
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let opt = Opts::parse();

    // Commands that don't talk a device
    if matches!(opt.command, Command::List) {
        return list_available_devices();
    }

    let mut devices = rockusb::nusb::devices()?;
    let info = if let Some(dev) = opt.device {
        devices
            .find(|d| d.busnum() == dev.bus_number && d.device_address() == dev.address)
            .ok_or_else(|| anyhow!("Specified device not found"))?
    } else {
        let mut devices: Vec<_> = devices.collect();
        match devices.len() {
            0 => Err(anyhow!("No devices found")),
            1 => Ok(devices.pop().unwrap()),
            _ => {
                drop(devices);
                let _ = list_available_devices();
                println!();
                Err(anyhow!(
                    "Please select a specific device using the -d option"
                ))
            }
        }?
    };

    let device = Device::from_usb_device_info(info)?;
    let device = ExampleDeviceAsync::new(device);
    opt.command.run_async(device).await
}
