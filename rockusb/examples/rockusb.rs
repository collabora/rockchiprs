mod common;
use anyhow::{Result, anyhow};
use clap::Parser;
use common::ExampleDevice;
use rockusb::libusb::DeviceUnavalable;

fn list_available_devices() -> Result<()> {
    let devices = rockusb::libusb::Devices::new()?;
    println!("Available rockchip devices");
    for d in devices.iter() {
        match d {
            Ok(d) => println!("* {:?}", d.transport().handle().device()),
            Err(DeviceUnavalable { device, error }) => {
                println!("* {:?} - Unavailable: {}", device, error)
            }
        }
    }

    Ok(())
}

fn main() -> Result<()> {
    let opt = common::Opts::parse();

    // Commands that don't talk a device
    if matches!(opt.command, common::Command::List) {
        return list_available_devices();
    }

    let devices = rockusb::libusb::Devices::new()?;
    let device = if let Some(dev) = opt.device {
        devices
            .iter()
            .find(|d| match d {
                Ok(device) => {
                    device.bus_number() == dev.bus_number && device.address() == dev.address
                }
                Err(DeviceUnavalable { device, .. }) => {
                    device.bus_number() == dev.bus_number && device.address() == dev.address
                }
            })
            .ok_or_else(|| anyhow!("Specified device not found"))?
    } else {
        let mut devices: Vec<_> = devices.iter().collect();
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
    }?;

    let device = ExampleDevice::new(device);

    opt.command.run(device)
}
