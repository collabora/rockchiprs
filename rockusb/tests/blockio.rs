use rockusb::protocol::SECTOR_SIZE;
use rockusb_mock::{MockDevice, MockDeviceConfig};

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
