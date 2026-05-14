use rockusb::protocol::SECTOR_SIZE;
use rockusb_mock::{MockDevice, MockDeviceConfig};
use std::io::{Read, Seek, SeekFrom, Write};

fn byte_at(pos: usize) -> u8 {
    // Modulo a prime so the pattern doesn't align with sector boundaries,
    // ensuring each sector has unique content.
    (pos % 13) as u8
}

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
    const PRIME: usize = 7;
    const SECTORS: usize = 4096;

    let flash = (0..SECTORS * SECTOR_SIZE as usize).map(byte_at).collect();
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
            assert_eq!(byte_at(pos), *b, "unexpected change, pos {pos}");
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

#[test]
fn io_size_matches_flash() {
    const SECTORS: usize = 4;
    let mut mock = MockDevice::new(MockDeviceConfig {
        flash: vec![0; SECTORS * SECTOR_SIZE as usize],
        ..MockDeviceConfig::default()
    })
    .unwrap();
    let device = mock.device();
    let io = device.io().unwrap();
    assert_eq!(io.size(), SECTORS as u64 * SECTOR_SIZE);
}

#[test]
fn simple_linear_read() {
    const SECTORS: usize = 4;
    let flash: Vec<u8> = (0..SECTORS * SECTOR_SIZE as usize).map(byte_at).collect();
    let expected = flash.clone();
    let mut mock = MockDevice::new(MockDeviceConfig {
        flash,
        ..MockDeviceConfig::default()
    })
    .unwrap();
    let buf = {
        let device = mock.device();
        let mut io = device.io().unwrap();
        let mut buf = vec![0u8; SECTORS * SECTOR_SIZE as usize];
        io.read_exact(&mut buf).unwrap();
        buf
    };
    assert_eq!(buf, expected);
    assert_eq!(mock.flash(), expected, "read must not modify flash content");
}

#[test]
fn read_write_roundtrip() {
    const SECTORS: usize = 4;
    let data: Vec<u8> = (0..SECTORS * SECTOR_SIZE as usize).map(byte_at).collect();
    let mut mock = MockDevice::new(MockDeviceConfig {
        flash: vec![0; SECTORS * SECTOR_SIZE as usize],
        ..MockDeviceConfig::default()
    })
    .unwrap();
    let device = mock.device();
    let mut io = device.io().unwrap();
    io.write_all(&data).unwrap();
    io.seek(SeekFrom::Start(0)).unwrap();
    let mut buf = vec![0u8; SECTORS * SECTOR_SIZE as usize];
    io.read_exact(&mut buf).unwrap();
    assert_eq!(buf, data);
}

#[test]
fn read_at_eof_returns_zero() {
    let mut mock = MockDevice::new(MockDeviceConfig {
        flash: vec![0; SECTOR_SIZE as usize],
        ..MockDeviceConfig::default()
    })
    .unwrap();
    let device = mock.device();
    let mut io = device.io().unwrap();
    io.seek(SeekFrom::End(0)).unwrap();
    let mut buf = [0u8; 16];
    assert_eq!(io.read(&mut buf).unwrap(), 0);
}

#[test]
fn seek_from_start() {
    // Step through flash in multiples of a prime. Because the prime is
    // coprime with SECTOR_SIZE the offsets land at a different position
    // within each sector on every pass, and each iteration starts from a
    // different cursor position (offset+1 after the read).
    const PRIME: usize = 7;
    const SECTORS: usize = 4;
    let flash: Vec<u8> = (0..SECTORS * SECTOR_SIZE as usize).map(byte_at).collect();
    let mut mock = MockDevice::new(MockDeviceConfig {
        flash,
        ..MockDeviceConfig::default()
    })
    .unwrap();
    let device = mock.device();
    let mut io = device.io().unwrap();
    let mut byte = [0u8; 1];

    for i in 0..SECTORS * SECTOR_SIZE as usize / PRIME {
        let offset = (i * PRIME) as u64;
        assert_eq!(io.seek(SeekFrom::Start(offset)).unwrap(), offset);
        io.read_exact(&mut byte).unwrap();
        assert_eq!(byte[0], byte_at(offset as usize));
    }
}

#[test]
fn seek_from_end() {
    // Step back from the end in multiples of a prime for the same reason as
    // seek_from_start: varied sub-sector positions from varied cursor locations.
    // Start at i=1 so we skip the trivial seek-to-EOF case.
    const PRIME: usize = 7;
    const SECTORS: usize = 4;
    let size = SECTORS as u64 * SECTOR_SIZE;
    let flash: Vec<u8> = (0..SECTORS * SECTOR_SIZE as usize).map(byte_at).collect();
    let mut mock = MockDevice::new(MockDeviceConfig {
        flash,
        ..MockDeviceConfig::default()
    })
    .unwrap();
    let device = mock.device();
    let mut io = device.io().unwrap();
    let mut byte = [0u8; 1];

    for i in 1..=SECTORS * SECTOR_SIZE as usize / PRIME {
        let offset = (i * PRIME) as u64;
        let expected_pos = size - offset;
        assert_eq!(
            io.seek(SeekFrom::End(-(offset as i64))).unwrap(),
            expected_pos
        );
        io.read_exact(&mut byte).unwrap();
        assert_eq!(byte[0], byte_at(expected_pos as usize));
    }
}

#[test]
fn seek_before_start_clamps_to_zero() {
    let mut mock = MockDevice::new(MockDeviceConfig {
        flash: vec![0; 2 * SECTOR_SIZE as usize],
        ..MockDeviceConfig::default()
    })
    .unwrap();
    let device = mock.device();
    let mut io = device.io().unwrap();
    assert_eq!(io.seek(SeekFrom::Current(i64::MIN)).unwrap(), 0);
}

#[test]
fn seek_past_end_clamps_to_size() {
    const SECTORS: usize = 2;
    let size = SECTORS as u64 * SECTOR_SIZE;
    let mut mock = MockDevice::new(MockDeviceConfig {
        flash: vec![0; SECTORS * SECTOR_SIZE as usize],
        ..MockDeviceConfig::default()
    })
    .unwrap();
    let device = mock.device();
    let mut io = device.io().unwrap();
    assert_eq!(io.seek(SeekFrom::Start(u64::MAX)).unwrap(), size);
}

#[test]
fn write_past_end_returns_error() {
    let mut mock = MockDevice::new(MockDeviceConfig {
        flash: vec![0; SECTOR_SIZE as usize],
        ..MockDeviceConfig::default()
    })
    .unwrap();
    let device = mock.device();
    let mut io = device.io().unwrap();
    io.seek(SeekFrom::End(0)).unwrap();
    assert!(io.write(&[0u8]).is_err());
}
