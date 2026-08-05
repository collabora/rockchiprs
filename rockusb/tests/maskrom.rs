use crc::Crc;
use rockusb_mock::MockDevice;

fn crc16(bytes: &[u8]) -> [u8; 2] {
    Crc::<u16>::new(&crc::CRC_16_IBM_3740)
        .checksum(bytes)
        .to_be_bytes()
}

#[test]
fn simple_maskrom() {
    let mut mock = MockDevice::default();
    let device = mock.device();

    device.erase_lba(0, 1).unwrap();
    device.write_maskrom_area(0x471, b"sram").unwrap();
    device.write_maskrom_area(0x472, b"ddr").unwrap();

    // sram + crc16
    let area = mock.maskrom_area(0x471).unwrap();
    assert_eq!(area.len(), 6);
    assert_eq!(&area[..4], b"sram");
    assert_eq!(area[4..6], crc16(b"sram"));

    // ddr + crc16
    let area = mock.maskrom_area(0x472).unwrap();
    assert_eq!(area.len(), 5);
    assert_eq!(&area[..3], b"ddr");
    assert_eq!(area[3..5], crc16(b"ddr"));
}

#[test]
fn maskrom_4094_byte_payload_adds_dummy_write() {
    let mut mock = MockDevice::default();
    let device = mock.device();
    let payload = vec![0x11; 4094];

    device.write_maskrom_area(0x473, &payload).unwrap();

    let area = mock.maskrom_area(0x473).unwrap();
    assert_eq!(area.len(), 4097);
    assert_eq!(&area[..4094], payload.as_slice());
    assert_eq!(&area[4094..4096], crc16(&payload).as_slice());
    assert_eq!(area[4096], 0);
}

#[test]
fn maskrom_4095_byte_payload_pads_before_crc() {
    let mut mock = MockDevice::default();
    let device = mock.device();
    let payload = vec![0x22; 4095];

    device.write_maskrom_area(0x474, &payload).unwrap();

    let area = mock.maskrom_area(0x474).unwrap();
    assert_eq!(area.len(), 4098);
    assert_eq!(&area[..4095], payload.as_slice());
    assert_eq!(area[4095], 0);

    let mut padded = payload.clone();
    padded.push(0);
    assert_eq!(&area[4096..4098], crc16(&padded).as_slice());
}

#[test]
fn maskrom_4096_byte_payload_appends_crc() {
    let mut mock = MockDevice::default();
    let device = mock.device();
    let payload = vec![0x33; 4096];

    device.write_maskrom_area(0x475, &payload).unwrap();

    let area = mock.maskrom_area(0x475).unwrap();
    assert_eq!(area.len(), 4098);
    assert_eq!(&area[..4096], payload.as_slice());
    assert_eq!(&area[4096..4098], crc16(&payload).as_slice());
}

#[test]
fn maskrom_4097_byte_payload_spans_two_transfers() {
    let mut mock = MockDevice::default();
    let device = mock.device();
    let payload = vec![0x44; 4097];

    device.write_maskrom_area(0x476, &payload).unwrap();

    let area = mock.maskrom_area(0x476).unwrap();
    assert_eq!(area.len(), 4099);
    assert_eq!(&area[..4097], payload.as_slice());
    assert_eq!(&area[4097..4099], crc16(&payload).as_slice());
}
