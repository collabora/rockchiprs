hs functiosn
  -> highspeed

fs functions
  -> fullspeed

command status wrapper:
  struct bulk_cs_wrap {
  >-------__le32  signature;              /* Should = 'USBS' */
  >-------u32     tag;                    /* Same as original command */
  >-------__le32  residue;                /* Amount not transferred */
  >-------u8      status;                 /* See below */
                      good -> 0x0, fail -> 0x1
  };

-> 13 bytes

command block wrapper:
  struct fsg_bulk_cb_wrap {
  >-------__le32  signature;              /* Contains 'USBC' */
  >-------u32     tag;                    /* Unique per command id */
  >-------__le32  data_transfer_length;   /* Size of the data */
  >-------u8      flags;                  /* Direction in bit 7 */
  >-------u8      lun;                    /* lun (normally 0) */
  >-------u8      length;                 /* Of the CDB, <= MAX_COMMAND_SIZE */
  >-------u8      CDB[16];                /* Command Data Block */
  };

-> 31 bytes

uboot:
  `K_FW_TEST_UNIT_READY` = 0x00,
    -> reply echo
    -> reply csw 
      tag = tag
      residue = size (0?)
      status = good

  `K_FW_READ_FLASH_ID` = 0x01,
    read storage string - 5 bytes
    get csw tag, good;

  `K_FW_SET_DEVICE_ID` = 0x02,
    not supported by uboot

  `K_FW_TEST_BAD_BLOCK` = 0x03,
    not supported by uboot

  `K_FW_READ_10` = 0x04,
    not supported by uboot

  `K_FW_WRITE_10` = 0x05,
    not support by uboot

  `K_FW_ERASE_10` = 0x06,
    not supported by uboot 

  `K_FW_WRITE_SPARE` = 0x07,
    not support by uboot
  `K_FW_READ_SPARE` = 0x08,
    not support by uboot

  `K_FW_ERASE_10_FORCE` = 0x0b,
    not support by uboot

  `K_FW_GET_VERSION` = 0x0c,
    no supported by uboot

  `K_FW_LBA_READ_10` = 0x14,
     lba = get_unaligned_be32(&cbw->CDB[2]);
     sector count = get_unaligned_be16(&cbw->CDB[7])
     f_rkusb->ul_size = sector_count * f_rkusb->desc->blksz;

     transfersize = 4096 at once (block size)
     csw as last 




  `K_FW_LBA_WRITE_10` = 0x15,
  `K_FW_ERASE_SYS_DISK` = 0x16,
  `K_FW_SDRAM_READ_10` = 0x17,
  `K_FW_SDRAM_WRITE_10` = 0x18,
  `K_FW_SDRAM_EXECUTE` = 0x19,
  `K_FW_READ_FLASH_INFO` = 0x1A,
  `K_FW_GET_CHIP_VER` = 0x1B,
  `K_FW_LOW_FORMAT` = 0x1C,
  `K_FW_SET_RESET_FLAG` = 0x1E,
  `K_FW_SPI_READ_10` = 0x21,
  `K_FW_SPI_WRITE_10` = 0x22,
  `K_FW_LBA_ERASE_10` = 0x25,

  `K_FW_SESSION` = 0X30,
  `K_FW_RESET` = 0xff,

