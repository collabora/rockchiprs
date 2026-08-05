use std::collections::BTreeMap;

use rockusb::{
    device::{Device, Error, Transport},
    operation::{OperationSteps, UsbStep},
    protocol::{
        COMMAND_STATUS_BYTES, CommandBlock, CommandBlockParseError, CommandCode, CommandStatus,
        Direction, ResetOpcode, Status,
    },
};
use thiserror::Error;

const DEFAULT_SECTORS: usize = 128;
const DEFAULT_BLOCK_SIZE_SECTORS: u16 = 64;

#[derive(Debug, Clone)]
pub struct MockDeviceConfig {
    pub chip_info: [u8; 16],
    pub flash_id: [u8; 5],
    pub capability: [u8; 8],
    pub flash: Vec<u8>,
    pub block_size_sectors: u16,
    pub flash_info_extra: [u8; 5],
}

impl Default for MockDeviceConfig {
    fn default() -> Self {
        Self {
            chip_info: *b"rockusb-mock-v1!",
            flash_id: *b"MOCK!",
            capability: [0x09, 0, 0, 0, 0, 0, 0, 0],
            flash: vec![0xff; DEFAULT_SECTORS * rockusb::protocol::SECTOR_SIZE as usize],
            block_size_sectors: DEFAULT_BLOCK_SIZE_SECTORS,
            flash_info_extra: [0; 5],
        }
    }
}

pub struct MockDevice {
    device: Device<MockState>,
}

impl Default for MockDevice {
    fn default() -> Self {
        Self::new(MockDeviceConfig::default()).expect("default mock device config is valid")
    }
}

impl MockDevice {
    pub fn new(config: MockDeviceConfig) -> Result<Self, MockConfigError> {
        if !config
            .flash
            .len()
            .is_multiple_of(rockusb::protocol::SECTOR_SIZE as usize)
        {
            return Err(MockConfigError::FlashSize(config.flash.len()));
        }

        Ok(Self {
            device: rockusb::device::Device::new(MockState {
                chip_info: config.chip_info,
                flash_id: config.flash_id,
                capability: config.capability,
                flash: config.flash,
                block_size_sectors: config.block_size_sectors,
                flash_info_extra: config.flash_info_extra,
                maskrom_areas: BTreeMap::new(),
                last_reset: None,
                current_command: CommandState::None,
            }),
        })
    }

    pub fn device(&mut self) -> &mut Device<MockState> {
        &mut self.device
    }

    fn state(&self) -> &MockState {
        self.device.transport()
    }

    pub fn flash(&self) -> &[u8] {
        &self.state().flash
    }

    pub fn maskrom_area(&self, area: u16) -> Option<&[u8]> {
        self.state().maskrom_areas.get(&area).map(Vec::as_slice)
    }

    pub fn last_reset(&self) -> Option<ResetOpcode> {
        self.state().last_reset
    }
}

impl Transport for MockState {
    type TransportError = MockTransportError;

    fn handle_operation<O, T>(&mut self, mut operation: O) -> Result<T, Error<Self::TransportError>>
    where
        O: OperationSteps<T>,
    {
        loop {
            match operation.step() {
                UsbStep::WriteControl {
                    request_type,
                    request,
                    value,
                    index,
                    data,
                } => {
                    self.write_control(request_type, request, value, index, data)?;
                }
                UsbStep::WriteBulk { data } => {
                    self.write_bulk(data).map_err(Error::UsbError)?;
                }
                UsbStep::ReadBulk { data } => {
                    self.read_bulk(data).map_err(Error::UsbError)?;
                }
                UsbStep::Finished(result) => return result.map_err(Error::OperationError),
            }
        }
    }
}

#[derive(Debug)]
enum CommandState {
    // No current command
    None,
    // Waiting on I/O for current command
    IO(CommandBlock),
    // Waiting for status read for current command
    Status(CommandBlock, rockusb::protocol::Status),
}

impl CommandState {
    fn take(&mut self) -> Self {
        let mut t = CommandState::None;
        std::mem::swap(self, &mut t);
        t
    }
}

#[derive(Debug)]
pub struct MockState {
    chip_info: [u8; 16],
    flash_id: [u8; 5],
    capability: [u8; 8],
    flash: Vec<u8>,
    block_size_sectors: u16,
    flash_info_extra: [u8; 5],
    maskrom_areas: BTreeMap<u16, Vec<u8>>,
    last_reset: Option<ResetOpcode>,
    // Current command
    current_command: CommandState,
}

impl MockState {
    fn flash_info(&self) -> [u8; 11] {
        let mut info = [0u8; 11];
        let sectors = (self.flash.len() / rockusb::protocol::SECTOR_SIZE as usize) as u32;
        info[0..4].copy_from_slice(&sectors.to_le_bytes());
        info[4..6].copy_from_slice(&self.block_size_sectors.to_le_bytes());
        info[6..].copy_from_slice(&self.flash_info_extra);
        info
    }

    fn write_control(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &[u8],
    ) -> Result<(), Error<MockTransportError>> {
        if request_type != 0x40 || request != 0xc || value != 0 {
            return Err(Error::UsbError(MockTransportError::UnsupportedControl {
                request_type,
                request,
                value,
            }));
        }

        self.maskrom_areas.entry(index).or_default().extend(data);
        Ok(())
    }

    fn handle_zero_length_command(
        &mut self,
        command: &CommandBlock,
    ) -> Result<(), MockTransportError> {
        match command.cd_code() {
            CommandCode::EraseLBA | CommandCode::EraseForce => {
                let range = self.flash_range(command.cd_address(), command.cd_length())?;
                self.flash[range].fill(0xff);
                Ok(())
            }
            CommandCode::DeviceReset => {
                self.last_reset = Some(
                    ResetOpcode::try_from(command.cd_opcode())
                        .map_err(|_| MockTransportError::InvalidReset(command.cd_opcode()))?,
                );
                Ok(())
            }
            code => Err(MockTransportError::UnknownCommand(code)),
        }
    }

    fn write_io(&mut self, command: &CommandBlock, data: &[u8]) -> Result<(), MockTransportError> {
        match command.cd_code() {
            CommandCode::WriteLBA => {
                let range = self.flash_range(command.cd_address(), command.cd_length())?;
                if data.len() != range.len() {
                    return Err(MockTransportError::InvalidTransferLength {
                        expected: range.len(),
                        actual: data.len(),
                    });
                }
                self.flash[range].copy_from_slice(data);
                Ok(())
            }
            _ => Err(MockTransportError::UnexpectedWrite),
        }
    }

    fn read_io(
        &mut self,
        command: &CommandBlock,
        data: &mut [u8],
    ) -> Result<(), MockTransportError> {
        match command.cd_code() {
            CommandCode::ReadFlashId => self.copy_reply(&self.flash_id, data),
            CommandCode::ReadFlashInfo => self.copy_reply(&self.flash_info(), data),
            CommandCode::ReadChipInfo => self.copy_reply(&self.chip_info, data),
            CommandCode::ReadCapability => self.copy_reply(&self.capability, data),
            CommandCode::ReadLBA => {
                let range = self.flash_range(command.cd_address(), command.cd_length())?;
                if data.len() != range.len() {
                    return Err(MockTransportError::InvalidTransferLength {
                        expected: range.len(),
                        actual: data.len(),
                    });
                }
                data.copy_from_slice(&self.flash[range]);
                Ok(())
            }
            _ => Err(MockTransportError::UnexpectedRead),
        }
    }

    fn write_bulk(&mut self, data: &[u8]) -> Result<(), MockTransportError> {
        match self.current_command.take() {
            CommandState::None => {
                let command = CommandBlock::from_bytes(data)?;
                if command.transfer_length() == 0 {
                    self.handle_zero_length_command(&command)?;
                    self.current_command = CommandState::Status(command, Status::SUCCESS);
                } else {
                    self.current_command = CommandState::IO(command);
                }
                Ok(())
            }
            CommandState::IO(command) => {
                if command.direction() == Direction::Out {
                    self.write_io(&command, data)?;
                    self.current_command = CommandState::Status(command.clone(), Status::SUCCESS);
                    Ok(())
                } else {
                    Err(MockTransportError::UnexpectedWrite)
                }
            }
            _ => Err(MockTransportError::UnexpectedWrite),
        }
    }

    fn read_bulk(&mut self, data: &mut [u8]) -> Result<(), MockTransportError> {
        match self.current_command.take() {
            CommandState::None => Err(MockTransportError::UnexpectedRead),
            CommandState::IO(command) => {
                if command.direction() == Direction::In {
                    self.read_io(&command, data)?;
                    self.current_command = CommandState::Status(command.clone(), Status::SUCCESS);
                    Ok(())
                } else {
                    Err(MockTransportError::UnexpectedRead)
                }
            }
            CommandState::Status(command, status) => {
                if data.len() == COMMAND_STATUS_BYTES {
                    let status = CommandStatus {
                        tag: command.tag(),
                        residue: 0,
                        status,
                    };
                    status.to_bytes(data);
                    Ok(())
                } else {
                    Err(MockTransportError::UnexpectedRead)
                }
            }
        }
    }

    fn copy_reply<const N: usize>(
        &self,
        reply: &[u8; N],
        data: &mut [u8],
    ) -> Result<(), MockTransportError> {
        if data.len() != reply.len() {
            return Err(MockTransportError::InvalidTransferLength {
                expected: reply.len(),
                actual: data.len(),
            });
        }
        data.copy_from_slice(reply);
        Ok(())
    }

    fn flash_range(
        &self,
        start_sector: u32,
        sectors: u16,
    ) -> Result<std::ops::Range<usize>, MockTransportError> {
        let sector_size = rockusb::protocol::SECTOR_SIZE as usize;
        let start = (start_sector as usize)
            .checked_mul(sector_size)
            .ok_or(MockTransportError::OutOfRange)?;
        let len = (sectors as usize)
            .checked_mul(sector_size)
            .ok_or(MockTransportError::OutOfRange)?;
        let end = start
            .checked_add(len)
            .ok_or(MockTransportError::OutOfRange)?;
        if end > self.flash.len() {
            return Err(MockTransportError::OutOfRange);
        }
        Ok(start..end)
    }

    pub fn flash(&self) -> &[u8] {
        &self.flash
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MockConfigError {
    #[error("flash size must be a multiple of 512 bytes, got {0}")]
    FlashSize(usize),
}

#[derive(Debug, Clone, Error)]
pub enum MockTransportError {
    #[error("Unexpected transfer length: expected {expected}, actual {actual}")]
    InvalidTransferLength { expected: usize, actual: usize },
    #[error("Unsupported Control: type: {request_type}, request: {request}, value: {value}")]
    UnsupportedControl {
        request_type: u8,
        request: u8,
        value: u16,
    },
    #[error("Invalid reset code: {0}")]
    InvalidReset(u8),
    #[error("Unknown Command: {0:?}")]
    UnknownCommand(CommandCode),
    #[error("Bulk read was not expected")]
    UnexpectedRead,
    #[error("Bulk write was not expected")]
    UnexpectedWrite,
    #[error("Failed to parse command block: {0}")]
    CommandBlockError(#[from] CommandBlockParseError),
    #[error("Outside of flash area")]
    OutOfRange,
}
