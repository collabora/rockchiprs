use rockusb::protocol::SECTOR_SIZE;
use rockusb_mock::{MockDevice, MockDeviceConfig};
use std::io::Write;

#[test]
fn erase_lba() {
    let mut mock = MockDevice::new(MockDeviceConfig {
        flash: vec![0x11; 2 * SECTOR_SIZE as usize],
        ..MockDeviceConfig::default()
    })
    .unwrap();
    let device = mock.device();

    device.erase_lba(0, 1).unwrap();
    let flash = mock.flash();
    assert!(
        flash[..SECTOR_SIZE as usize]
            .iter()
            .all(|byte| *byte == 0xff)
    );
    assert!(
        flash[SECTOR_SIZE as usize..]
            .iter()
            .all(|byte| *byte == 0x11)
    );
}

#[test]
fn simple_linear_write() {
    const SECTORS: usize = 4096;
    let mut mock = MockDevice::new(MockDeviceConfig {
        flash: vec![0x0; SECTORS * SECTOR_SIZE as usize],
        ..MockDeviceConfig::default()
    })
    .unwrap();
    let device = mock.device();
    let mut io = device.io().unwrap();
    for i in 0..(SECTORS * SECTOR_SIZE as usize) {
        io.write_all(&[(i & 0xff) as u8]).unwrap();
    }

    let flash = mock.flash();
    for (pos, b) in flash.iter().enumerate() {
        assert_eq!((pos & 0xff) as u8, *b, "unexpected value, pos {pos}");
    }
}
