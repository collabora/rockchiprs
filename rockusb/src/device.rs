use crate::{
    operation::OperationSteps,
    protocol::{Capability, ChipInfo, FlashId, FlashInfo, ResetOpcode},
};
use thiserror::Error;

#[derive(Debug, Clone, Eq, PartialEq, Error)]
/// Error type return by most [Device] method
pub enum Error<TE> {
    #[error("Usb error: {0}")]
    UsbError(TE),
    #[error("Operation error: {0}")]
    OperationError(#[from] crate::operation::UsbOperationError),
}

/// Device wrapper for rockusb operations
pub struct Device<Transport> {
    transport: Transport,
}

/// Trait to be implemented by backing transports
pub trait Transport {
    type TransportError;
    fn handle_operation<O, T>(&mut self, operation: O) -> Result<T, Error<Self::TransportError>>
    where
        O: OperationSteps<T>;
}

/// Result type return by most [Device] method
pub type DeviceResult<T, Trans> = Result<T, Error<<Trans as Transport>::TransportError>>;

impl<T> Device<T>
where
    T: Transport,
{
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    /// Get a reference to the underlying transport
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// retrieve SoC flash identifier
    pub fn flash_id(&mut self) -> DeviceResult<FlashId, T> {
        self.transport
            .handle_operation(crate::operation::flash_id())
    }

    /// retrieve SoC flash info
    pub fn flash_info(&mut self) -> DeviceResult<FlashInfo, T> {
        self.transport
            .handle_operation(crate::operation::flash_info())
    }

    /// retrieve SoC chip info
    pub fn chip_info(&mut self) -> DeviceResult<ChipInfo, T> {
        self.transport
            .handle_operation(crate::operation::chip_info())
    }

    /// retrieve SoC capability
    pub fn capability(&mut self) -> DeviceResult<Capability, T> {
        self.transport
            .handle_operation(crate::operation::capability())
    }

    /// read from the flash
    ///
    /// start_sector with [SECTOR_SIZE] sectors. the data to be read
    /// must be a multiple of [SECTOR_SIZE] bytes
    pub fn read_lba(&mut self, start_sector: u32, read: &mut [u8]) -> DeviceResult<u32, T> {
        self.transport
            .handle_operation(crate::operation::read_lba(start_sector, read))
            .map(|t| t.into())
    }

    /// Create operation to read an lba from the flash
    ///
    /// start_sector based on [SECTOR_SIZE] sectors. the data to be
    /// written must be a multiple of [SECTOR_SIZE] bytes
    pub fn write_lba(&mut self, start_sector: u32, write: &[u8]) -> DeviceResult<u32, T> {
        self.transport
            .handle_operation(crate::operation::write_lba(start_sector, write))
            .map(|t| t.into())
    }

    /// Write a specific area while in maskrom mode; typically 0x471 or 0x472 data as retrieved from a
    /// rockchip boot file
    pub fn write_maskrom_area(&mut self, area: u16, data: &[u8]) -> DeviceResult<(), T> {
        self.transport
            .handle_operation(crate::operation::write_area(area, data))
    }

    /// Reset the device
    pub fn reset_device(&mut self, opcode: ResetOpcode) -> DeviceResult<(), T> {
        self.transport
            .handle_operation(crate::operation::reset_device(opcode))
    }
}
