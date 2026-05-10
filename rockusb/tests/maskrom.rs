use rockusb_mock::MockDevice;

#[test]
fn simple_maskrom() {
    let mut mock = MockDevice::default();
    let device = mock.device();

    device.erase_lba(0, 1).unwrap();
    device.write_maskrom_area(0x471, b"sram").unwrap();
    device.write_maskrom_area(0x472, b"ddr").unwrap();

    let area = mock.maskrom_area(0x471).unwrap();
    // sram + crc16
    assert_eq!(area, b"sram\x6e\x11");
    // ddr + crc16
    let area = mock.maskrom_area(0x472).unwrap();
    assert_eq!(area, b"ddr\x12\x0c");
}
