#![cfg_attr(docsrs, feature(doc_auto_cfg))]
#![doc = include_str!("../README.md")]

/// libusb transport implementation
#[cfg(feature = "libusb")]
pub mod libusb;
/// nusb transport implementation
#[cfg(feature = "nusb")]
pub mod nusb;
/// sans-io protocol implementations
///
/// This module contains all protocol logic; Each operation implements the [operation::OperationSteps]
/// trait which gives a transport a series of [operation::UsbStep] to execute to complete an
/// operation.
pub mod operation;
/// low-level usb protocol data structures
pub mod protocol;
