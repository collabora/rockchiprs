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

#[maybe_async_cfg::maybe(sync(keep_self), async(feature = "async"))]
/// Device wrapper for rockusb operations
pub struct Device<Transport> {
    transport: Transport,
}

#[maybe_async_cfg::maybe(sync(keep_self), async(feature = "async"))]
/// Trait to be implemented by backing transports
pub trait Transport {
    type TransportError;
    #[maybe_async_cfg::only_if(sync)]
    fn handle_operation<O, T>(&mut self, operation: O) -> Result<T, Error<Self::TransportError>>
    where
        O: OperationSteps<T>;
    #[maybe_async_cfg::only_if(async)]
    fn handle_operation<O, T>(
        &mut self,
        operation: O,
    ) -> impl Future<Output = Result<T, Error<Self::TransportError>>> + Send
    where
        O: OperationSteps<T> + Send,
        T: Send;
}

/// Result type return by most [Device] method
pub type DeviceResult<T, Trans> = Result<T, Error<<Trans as Transport>::TransportError>>;
#[cfg(feature = "async")]
/// Result type return by most [DeviceAsync] method
pub type DeviceResultAsync<T, Trans> = Result<T, Error<<Trans as TransportAsync>::TransportError>>;

#[maybe_async_cfg::maybe(
    sync(keep_self),
    async(
        feature = "async",
        idents(
            DeviceResult(async = "DeviceResultAsync"),
            Transport(async = "TransportAsync")
        )
    )
)]
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
    pub async fn flash_id(&mut self) -> DeviceResult<FlashId, T> {
        self.transport
            .handle_operation(crate::operation::flash_id())
            .await
    }

    /// retrieve SoC flash info
    pub async fn flash_info(&mut self) -> DeviceResult<FlashInfo, T> {
        self.transport
            .handle_operation(crate::operation::flash_info())
            .await
    }

    /// retrieve SoC chip info
    pub async fn chip_info(&mut self) -> DeviceResult<ChipInfo, T> {
        self.transport
            .handle_operation(crate::operation::chip_info())
            .await
    }

    /// retrieve SoC capability
    pub async fn capability(&mut self) -> DeviceResult<Capability, T> {
        self.transport
            .handle_operation(crate::operation::capability())
            .await
    }

    /// read from the flash
    ///
    /// start_sector with [SECTOR_SIZE] sectors. the data to be read
    /// must be a multiple of [SECTOR_SIZE] bytes
    pub async fn read_lba(&mut self, start_sector: u32, read: &mut [u8]) -> DeviceResult<u32, T> {
        self.transport
            .handle_operation(crate::operation::read_lba(start_sector, read))
            .await
            .map(|t| t.into())
    }

    /// Create operation to read an lba from the flash
    ///
    /// start_sector based on [SECTOR_SIZE] sectors. the data to be
    /// written must be a multiple of [SECTOR_SIZE] bytes
    pub async fn write_lba(&mut self, start_sector: u32, write: &[u8]) -> DeviceResult<u32, T> {
        self.transport
            .handle_operation(crate::operation::write_lba(start_sector, write))
            .await
            .map(|t| t.into())
    }

    /// Write a specific area while in maskrom mode; typically 0x471 or 0x472 data as retrieved from a
    /// rockchip boot file
    pub async fn write_maskrom_area(&mut self, area: u16, data: &[u8]) -> DeviceResult<(), T> {
        self.transport
            .handle_operation(crate::operation::write_area(area, data))
            .await
    }

    /// Reset the device
    pub async fn reset_device(&mut self, opcode: ResetOpcode) -> DeviceResult<(), T> {
        self.transport
            .handle_operation(crate::operation::reset_device(opcode))
            .await
    }
}
