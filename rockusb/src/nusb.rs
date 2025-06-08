use crate::operation::{OperationSteps, UsbStep};
pub use nusb::transfer::TransferError;
use nusb::{
    DeviceInfo,
    transfer::{ControlOut, ControlType, Recipient, RequestBuffer},
};
use thiserror::Error;

/// Error indicate a device is not available
#[derive(Debug, Error)]
#[error("Device is not available: {error}")]
pub struct DeviceUnavalable {
    #[from]
    pub error: nusb::Error,
}

/// List rockchip devices
pub fn devices() -> std::result::Result<impl Iterator<Item = DeviceInfo>, nusb::Error> {
    Ok(nusb::list_devices()?.filter(|d| d.vendor_id() == 0x2207))
}

impl From<TransferError> for crate::device::Error<TransferError> {
    fn from(value: TransferError) -> Self {
        Self::UsbError(value)
    }
}

/// nusb based Transport for rockusb operation
pub struct Transport {
    interface: nusb::Interface,
    ep_in: u8,
    ep_out: u8,
}

impl crate::device::TransportAsync for Transport {
    type TransportError = TransferError;
    async fn handle_operation<O, T>(
        &mut self,
        mut operation: O,
    ) -> crate::device::DeviceResultAsync<T, Self>
    where
        O: OperationSteps<T>,
    {
        loop {
            let step = operation.step();
            match step {
                UsbStep::WriteBulk { data } => {
                    let _written = self
                        .interface
                        .bulk_out(self.ep_out, data.to_vec())
                        .await
                        .into_result()?;
                }
                UsbStep::ReadBulk { data } => {
                    let req = RequestBuffer::new(data.len());
                    let read = self
                        .interface
                        .bulk_in(self.ep_in, req)
                        .await
                        .into_result()?;
                    data.copy_from_slice(&read);
                }
                UsbStep::WriteControl {
                    request_type,
                    request,
                    value,
                    index,
                    data,
                } => {
                    let (control_type, recipient) = (
                        match (request_type >> 5) & 0x03 {
                            0 => ControlType::Standard,
                            1 => ControlType::Class,
                            2 => ControlType::Vendor,
                            _ => ControlType::Standard,
                        },
                        match request_type & 0x1f {
                            0 => Recipient::Device,
                            1 => Recipient::Interface,
                            2 => Recipient::Endpoint,
                            3 => Recipient::Other,
                            _ => Recipient::Device,
                        },
                    );
                    let data = ControlOut {
                        control_type,
                        recipient,
                        request,
                        value,
                        index,
                        data,
                    };
                    self.interface.control_out(data).await.into_result()?;
                }
                UsbStep::Finished(r) => break r.map_err(|e| e.into()),
            }
        }
    }
}

impl Transport {
    fn new(
        device: nusb::Device,
        interface: u8,
        ep_in: u8,
        ep_out: u8,
    ) -> std::result::Result<Self, DeviceUnavalable> {
        let interface = device.claim_interface(interface)?;
        Ok(Self {
            interface,
            ep_in,
            ep_out,
        })
    }
}

pub type Device = crate::device::DeviceAsync<Transport>;
impl Device {
    /// Create a new transport from a device info
    pub fn from_usb_device_info(
        info: nusb::DeviceInfo,
    ) -> std::result::Result<Self, DeviceUnavalable> {
        let device = info.open()?;
        Self::from_usb_device(device)
    }

    /// Create a new transport from an existing device
    pub fn from_usb_device(device: nusb::Device) -> std::result::Result<Self, DeviceUnavalable> {
        for config in device.clone().configurations() {
            for interface in config.interface_alt_settings() {
                let output = interface.endpoints().find(|e| {
                    e.direction() == nusb::transfer::Direction::Out
                        && e.transfer_type() == nusb::transfer::EndpointType::Bulk
                });
                let input = interface.endpoints().find(|e| {
                    e.direction() == nusb::transfer::Direction::In
                        && e.transfer_type() == nusb::transfer::EndpointType::Bulk
                });

                if let (Some(input), Some(output)) = (input, output) {
                    return Ok(Device::new(Transport::new(
                        device,
                        interface.interface_number(),
                        input.address(),
                        output.address(),
                    )?));
                }
            }
        }
        Err(DeviceUnavalable {
            error: nusb::Error::new(std::io::ErrorKind::NotFound, "Device not found"),
        })
    }
}
