use std::time::Duration;

use crate::operation::{OperationSteps, UsbStep};
use rusb::{DeviceHandle, GlobalContext};
use thiserror::Error;

/// Error indicate a device is not available
#[derive(Debug, Clone, Eq, PartialEq, Error)]
#[error("Device is not available: {device:?} {error}")]
pub struct DeviceUnavalable {
    pub device: rusb::Device<GlobalContext>,
    #[source]
    pub error: rusb::Error,
}

/// Rockchip devices
pub struct Devices {
    devices: rusb::DeviceList<GlobalContext>,
}

impl Devices {
    pub fn new() -> Result<Self> {
        let devices = rusb::DeviceList::new()?;
        Ok(Self { devices })
    }

    /// Create an Iterator over found Rockchip device
    pub fn iter(&self) -> DevicesIter {
        let iter = self.devices.iter();
        DevicesIter { iter }
    }
}

/// Iterator over found Rockchip device
pub struct DevicesIter<'a> {
    iter: rusb::Devices<'a, GlobalContext>,
}

impl Iterator for DevicesIter<'_> {
    type Item = std::result::Result<Device, DeviceUnavalable>;

    fn next(&mut self) -> Option<Self::Item> {
        for device in self.iter.by_ref() {
            let desc = match device.device_descriptor() {
                Ok(desc) => desc,
                _ => continue,
            };
            if desc.vendor_id() != 0x2207 {
                continue;
            }
            let handle = match device.open() {
                Ok(handle) => handle,
                Err(error) => return Some(Err(DeviceUnavalable { device, error })),
            };

            return Some(Device::from_usb_device(handle));
        }
        None
    }
}

/// libusb based Transport
pub struct Transport {
    handle: DeviceHandle<rusb::GlobalContext>,
    ep_in: u8,
    ep_out: u8,
}

impl Transport {
    pub fn handle(&self) -> &DeviceHandle<rusb::GlobalContext> {
        &self.handle
    }
}

impl crate::device::Transport for Transport {
    type TransportError = rusb::Error;
    fn handle_operation<O, T>(&mut self, mut operation: O) -> crate::device::DeviceResult<T, Self>
    where
        O: OperationSteps<T>,
    {
        loop {
            let step = operation.step();
            match step {
                UsbStep::WriteBulk { data } => {
                    let _written =
                        self.handle
                            .write_bulk(self.ep_out, data, Duration::from_secs(5))?;
                }
                UsbStep::ReadBulk { data } => {
                    let _read = self
                        .handle
                        .read_bulk(self.ep_in, data, Duration::from_secs(5))?;
                }
                UsbStep::Finished(r) => break r.map_err(|e| e.into()),
                UsbStep::WriteControl {
                    request_type,
                    request,
                    value,
                    index,
                    data,
                } => {
                    self.handle.write_control(
                        request_type,
                        request,
                        value,
                        index,
                        data,
                        Duration::from_secs(5),
                    )?;
                }
            }
        }
    }
}

impl From<rusb::Error> for crate::device::Error<rusb::Error> {
    fn from(value: rusb::Error) -> Self {
        Self::UsbError(value)
    }
}

pub type Device = crate::device::Device<Transport>;
type Result<T> = crate::device::DeviceResult<T, Transport>;
impl Device {
    fn new_libusb(
        handle: DeviceHandle<rusb::GlobalContext>,
        interface: u8,
        ep_in: u8,
        ep_out: u8,
    ) -> std::result::Result<Self, DeviceUnavalable> {
        handle
            .claim_interface(interface)
            .map_err(|error| DeviceUnavalable {
                device: handle.device(),
                error,
            })?;
        Ok(Self::new(Transport {
            handle,
            ep_in,
            ep_out,
        }))
    }

    /// Create a new transport from an exist device handle
    pub fn from_usb_device(
        handle: rusb::DeviceHandle<GlobalContext>,
    ) -> std::result::Result<Self, DeviceUnavalable> {
        let device = handle.device();
        let desc = device
            .device_descriptor()
            .map_err(|error| DeviceUnavalable {
                device: device.clone(),
                error,
            })?;
        for c in 0..desc.num_configurations() {
            let config = device
                .config_descriptor(c)
                .map_err(|error| DeviceUnavalable {
                    device: device.clone(),
                    error,
                })?;
            for i in config.interfaces() {
                for i_desc in i.descriptors() {
                    let output = i_desc.endpoint_descriptors().find(|e| {
                        e.direction() == rusb::Direction::Out
                            && e.transfer_type() == rusb::TransferType::Bulk
                    });
                    let input = i_desc.endpoint_descriptors().find(|e| {
                        e.direction() == rusb::Direction::In
                            && e.transfer_type() == rusb::TransferType::Bulk
                    });

                    if let (Some(input), Some(output)) = (input, output) {
                        return Self::new_libusb(
                            handle,
                            i_desc.setting_number(),
                            input.address(),
                            output.address(),
                        );
                    }
                }
            }
        }
        Err(DeviceUnavalable {
            device,
            error: rusb::Error::NotFound,
        })
    }

    pub fn bus_number(&self) -> u8 {
        self.transport().handle.device().bus_number()
    }

    /// Get the bus address of the current device
    pub fn address(&self) -> u8 {
        self.transport().handle.device().address()
    }
}
