use protocol::{CommandBlock, Direction};
use thiserror::Error;

pub mod bootfile;
pub mod protocol;

#[derive(Debug, Clone, Eq, PartialEq, Error)]
pub enum RockUsbOperationError {
    #[error("Setup good error types")]
    TODO,
}

pub enum MaskRomStep<'a> {
    WriteControl {
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &'a [u8],
    },
    FillData {
        data: &'a mut [u8],
    },
}

#[derive(Debug, Eq, PartialEq)]
pub enum UsbStep<'a, T> {
    WriteBulk { data: &'a [u8] },
    ReadBulk { data: &'a mut [u8] },
    Finished(Result<T, RockUsbOperationError>),
}

enum Operation<T> {
    CommandBlock,
    IO,
    CommandStatus,
    Final(Result<T, RockUsbOperationError>),
}

pub struct UsbOperation<'a, T, const N: usize> {
    command: CommandBlock,
    bytes: [u8; N],
    data: Option<&'a mut [u8]>,
    next: Operation<T>,
}

impl<'a, T, const N: usize> UsbOperation<'a, T, N>
where
    T: for<'b> TryFrom<&'b [u8]> + 'static,
    T: std::fmt::Debug,
{
    fn new(command: CommandBlock) -> Self {
        Self {
            command,
            bytes: [0u8; N],
            data: None,
            next: Operation::CommandBlock,
        }
    }

    fn io_data(&mut self) -> &mut [u8] {
        if let Some(data) = self.data.as_mut() {
            data
        } else {
            &mut self.bytes
        }
    }

    pub fn step(&mut self) -> UsbStep<T> {
        let mut next = Operation::CommandBlock;
        std::mem::swap(&mut self.next, &mut next);
        match next {
            Operation::CommandBlock => {
                let len = self.command.to_bytes(&mut self.bytes);
                self.next = Operation::IO;
                UsbStep::WriteBulk {
                    data: &self.bytes[..len],
                }
            }
            Operation::IO => {
                self.next = Operation::CommandStatus;
                let len = self.command.transfer_length() as usize;
                match self.command.direction() {
                    Direction::Out => UsbStep::WriteBulk {
                        data: &self.io_data()[..len],
                    },
                    Direction::In => UsbStep::ReadBulk {
                        data: &mut self.io_data()[..len],
                    },
                }
            }
            Operation::CommandStatus => {
                let len = self.command.transfer_length() as usize;
                self.next = Operation::Final(
                    T::try_from(&self.bytes[..len]).map_err(|_e| RockUsbOperationError::TODO),
                );
                UsbStep::ReadBulk {
                    data: &mut self.bytes[..protocol::COMMAND_STATUS_BYTES],
                }
            }
            Operation::Final(r) => UsbStep::Finished(r),
        }
    }
}

pub fn chip_info() -> UsbOperation<'static, [u8; 16], 31> {
    UsbOperation::new(CommandBlock::chip_info())
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn chip_info_operation() {
        let mut o = chip_info();
        let cb = CommandBlock::chip_info();
        let mut cb_bytes = [0u8; protocol::COMMAND_BLOCK_BYTES];
        cb.to_bytes(&mut cb_bytes);
        // Write the command block
        let tag = match o.step() {
            UsbStep::WriteBulk { data } if data.len() == protocol::COMMAND_BLOCK_BYTES => {
                [data[0], data[1], data[2], data[3]]
            }
            o => panic!("Unexpected step: {:?}", o),
        };

        // Reading the chip info
        match o.step() {
            UsbStep::ReadBulk { data } if data.len() as u32 == cb.transfer_length() => {
                data.fill(0);
                /* 3588 */
                data[0] = 0x38;
                data[1] = 0x38;
                data[2] = 0x35;
                data[3] = 0x33;
            }
            o => panic!("Unexpected step: {:?}", o),
        }

        // reading status
        match o.step() {
            UsbStep::ReadBulk { data } if data.len() == protocol::COMMAND_STATUS_BYTES => {
                data.fill(0);
                /* tag */
                data[0] = tag[0];
                data[1] = tag[1];
                data[2] = tag[2];
                data[3] = tag[3];
                // status good
                data[12] = 0;
            }
            o => panic!("Unexpected step: {:?}", o),
        }

        match o.step() {
            UsbStep::Finished(Ok(info)) => {
                assert_eq!(
                    info,
                    [
                        0x38u8, 0x38, 0x35, 0x33, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
                        0x0, 0x0
                    ]
                )
            }
            o => panic!("Unexpected step: {:?}", o),
        }
    }
}
