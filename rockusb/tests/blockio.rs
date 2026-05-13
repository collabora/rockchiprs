use rockusb::protocol::SECTOR_SIZE;
use rockusb_mock::{MockDevice, MockDeviceConfig};
use std::io::{Seek, SeekFrom, Write};

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

#[test]
fn simple_seeking_write() {
    // Write one byte at a time, each at a prime offset.
    const PRIME: usize = 97;
    const SECTORS: usize = 4096;

    fn val_at_pos(pos: usize) -> u8 {
        // modulo a prime so the sectors aren't all the same;
        (pos % 13) as u8
    }

    let flash = (0..SECTORS * SECTOR_SIZE as usize)
        .map(val_at_pos)
        .collect();
    let mut mock = MockDevice::new(MockDeviceConfig {
        flash,
        ..MockDeviceConfig::default()
    })
    .unwrap();
    let device = mock.device();
    let mut io = device.io().unwrap();
    for i in 0..=(SECTORS * SECTOR_SIZE as usize) / PRIME {
        io.write_all(&[(i & 0xff) as u8]).unwrap();
        io.seek(SeekFrom::Current((PRIME - 1) as i64)).unwrap();
    }

    let flash = mock.flash();
    for (pos, b) in flash.iter().enumerate() {
        if pos.is_multiple_of(PRIME) {
            assert_eq!(
                ((pos / PRIME) & 0xff) as u8,
                *b,
                "unexpected value, pos {pos}"
            );
        } else {
            assert_eq!(val_at_pos(pos), *b, "unexpected change, pos {pos}");
        }
    }
}

#[test]
fn flush() {
    let mut mock = MockDevice::new(MockDeviceConfig {
        flash: vec![13; 2 * SECTOR_SIZE as usize],
        ..MockDeviceConfig::default()
    })
    .unwrap();
    let device = mock.device();
    let mut io = device.io().unwrap();

    // Write just half a sector, this shouldn't be written back
    io.write_all(&[99; (SECTOR_SIZE / 2) as usize]).unwrap();

    let flash = io.inner().transport().flash();
    for (pos, b) in flash.iter().enumerate() {
        assert_eq!(13, *b, "unexpected value, pos {pos}");
    }

    // Flush, data should now be on storage
    io.flush().unwrap();
    let flash = mock.flash();
    for (pos, b) in flash.iter().enumerate() {
        let expected = if pos < (SECTOR_SIZE / 2) as usize {
            99
        } else {
            13
        };
        assert_eq!(expected, *b, "unexpected value, pos {pos}");
    }
}
