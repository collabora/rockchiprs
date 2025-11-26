use std::time::Duration;

use crate::operation::{OperationSteps, UsbStep};
pub use nusb::transfer::TransferError;
use nusb::{
    DeviceInfo, MaybeFuture,
    transfer::{Bulk, Buffer, ControlOut, ControlType, In, Out, Recipient},
};
use thiserror::Error;

/// Error indicate a device is not available
#[derive(Debug, Error)]
pub enum DeviceUnavalable {
    #[error("Device is not available: {0}")]
    UsbError(#[from] nusb::Error),
    #[error("Device not found")]
    NotFound,
}

impl DeviceUnavalable {
    /// Create a "not found" error
    pub fn not_found() -> Self {
        DeviceUnavalable::NotFound
    }
}

/// List rockchip devices
pub fn devices() -> std::result::Result<impl Iterator<Item = DeviceInfo>, nusb::Error> {
    Ok(nusb::list_devices().wait()?.filter(|d| d.vendor_id() == 0x2207))
}

impl From<TransferError> for crate::device::Error<TransferError> {
    fn from(value: TransferError) -> Self {
        Self::UsbError(value)
    }
}

/// nusb based Transport for rockusb operation
pub struct Transport {
    interface: nusb::Interface,
    ep_in: nusb::Endpoint<Bulk, In>,
    ep_out: nusb::Endpoint<Bulk, Out>,
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
        // Default timeout for USB operations
        const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

        loop {
            let step = operation.step();
            match step {
                UsbStep::WriteBulk { data } => {
                    let buf: Buffer = data.to_vec().into();
                    self.ep_out.submit(buf);
                    let completion = self.ep_out.next_complete().await;
                    completion.into_result()?;
                }
                UsbStep::ReadBulk { data } => {
                    // For IN transfers, requested_len must be a multiple of max_packet_size
                    let max_packet_size = self.ep_in.max_packet_size();
                    let requested_len = ((data.len() + max_packet_size - 1) / max_packet_size) * max_packet_size;
                    let buf = Buffer::new(requested_len);
                    self.ep_in.submit(buf);
                    let completion = self.ep_in.next_complete().await;
                    let result_buf = completion.into_result()?;
                    data.copy_from_slice(&result_buf[..data.len()]);
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
                    self.interface.control_out(data, DEFAULT_TIMEOUT).await?;
                }
                UsbStep::Finished(r) => break r.map_err(|e| e.into()),
            }
        }
    }
}

impl Transport {
    fn new(
        interface: nusb::Interface,
        ep_in: nusb::Endpoint<Bulk, In>,
        ep_out: nusb::Endpoint<Bulk, Out>,
    ) -> Self {
        Self {
            interface,
            ep_in,
            ep_out,
        }
    }
}

pub type Device = crate::device::DeviceAsync<Transport>;
impl Device {
    /// Create a new transport from a device info
    pub fn from_usb_device_info(
        info: nusb::DeviceInfo,
    ) -> std::result::Result<Self, DeviceUnavalable> {
        let device = info.open().wait()?;
        Self::from_usb_device(device)
    }

    /// Create a new transport from an existing device
    pub fn from_usb_device(device: nusb::Device) -> std::result::Result<Self, DeviceUnavalable> {
        for config in device.clone().configurations() {
            for iface_setting in config.interface_alt_settings() {
                let output = iface_setting.endpoints().find(|e| {
                    e.direction() == nusb::transfer::Direction::Out
                        && e.transfer_type() == nusb::descriptors::TransferType::Bulk
                });
                let input = iface_setting.endpoints().find(|e| {
                    e.direction() == nusb::transfer::Direction::In
                        && e.transfer_type() == nusb::descriptors::TransferType::Bulk
                });

                if let (Some(input), Some(output)) = (input, output) {
                    let interface = device.claim_interface(iface_setting.interface_number()).wait()?;
                    let ep_in = interface.endpoint::<Bulk, In>(input.address())?;
                    let ep_out = interface.endpoint::<Bulk, Out>(output.address())?;
                    return Ok(Device::new(Transport::new(interface, ep_in, ep_out)));
                }
            }
        }
        Err(DeviceUnavalable::not_found())
    }
}
