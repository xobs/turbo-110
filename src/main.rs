use futures_lite::future::block_on;
use nusb::transfer::{ControlIn, ControlOut};
use std::time::Duration;
use usb_util::InterfaceExt;

mod usb_util;

const CMSIS_DAP_2_MINIMUM: u32 = 0x03_00_00_08;
const CONFIGURATION_SIZE: u16 = 16384;

#[derive(Debug)]
pub(crate) struct Xds110DfuDeviceMatch {
    vid: u16,
    pid: u16,
}

pub(crate) const XDS110_DFU_DEVICES: &[Xds110DfuDeviceMatch] = &[Xds110DfuDeviceMatch {
    vid: 0x1cbe,
    pid: 0x00ff,
}];

#[derive(Debug)]
pub(crate) struct Xds110UsbDeviceMatch {
    vid: u16,
    pid: u16,
    epin: u8,
    epout: u8,
    interface: u8,
}

pub(crate) const XDS110_USB_DEVICES: &[Xds110UsbDeviceMatch] = &[
    Xds110UsbDeviceMatch {
        vid: 0x0451,
        pid: 0xbef3,
        epin: 0x83,
        epout: 0x02,
        interface: 2,
    },
    Xds110UsbDeviceMatch {
        vid: 0x0451,
        pid: 0xbef4,
        epin: 0x83,
        epout: 0x02,
        interface: 2,
    },
    Xds110UsbDeviceMatch {
        vid: 0x1cbe,
        pid: 0x02a5,
        epin: 0x81,
        epout: 0x01,
        interface: 0,
    },
];

pub struct Xds110UsbDevice {
    device_handle: nusb::Interface,
    epout: u8,
    epin: u8,
}

pub struct Xds110DfuDevice {
    device_handle: nusb::Device,
    packet_count: u16,
}

impl Xds110UsbDevice {
    pub fn reboot_to_dfu(self) -> Result<(), std::io::Error> {
        // Send the "Reboot to DFU mode" packet.
        self.device_handle.write_bulk(
            self.epout,
            &[0x2a, 0x01, 0x00, 0x26],
            Duration::from_secs(1),
        )?;
        Ok(())
    }
    pub fn firmware_version(&self) -> Result<u32, std::io::Error> {
        let timeout = Duration::from_millis(100);
        self.device_handle
            .write_bulk(self.epout, &[0x2a, 0x01, 0x00, 0x03], timeout)?;
        let mut version = [0u8; 13];
        let response = self
            .device_handle
            .read_bulk(self.epin, &mut version, timeout)?;
        if response < 11 {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        Ok(u32::from_le_bytes(version[7..11].try_into().unwrap()))
    }
}

impl Xds110DfuDevice {
    /// Ensure the target speaks the Tiva DFU binary protocol
    pub fn ensure_binary_protocol(&self) -> Result<(), nusb::transfer::TransferError> {
        block_on(self.device_handle.control_in(ControlIn {
            control_type: nusb::transfer::ControlType::Class,
            recipient: nusb::transfer::Recipient::Interface,
            request: 0x42,
            value: 0x23,
            index: 0,
            length: 4,
        }))
        .into_result()?;
        Ok(())
    }

    /// This must be called after every operation
    fn get_dfu_status(&self) -> Result<Vec<u8>, nusb::transfer::TransferError> {
        block_on(self.device_handle.control_in(ControlIn {
            control_type: nusb::transfer::ControlType::Class,
            recipient: nusb::transfer::Recipient::Interface,
            request: 3,
            value: 0,
            index: 0,
            length: 6,
        }))
        .into_result()
    }

    pub fn read_configuration(&mut self) -> Result<Vec<u8>, nusb::transfer::TransferError> {
        // DFU_GETSTATUS
        println!("Getting DFU status...");
        let status = block_on(self.device_handle.control_in(ControlIn {
            control_type: nusb::transfer::ControlType::Class,
            recipient: nusb::transfer::Recipient::Interface,
            request: 3,
            value: self.packet_count,
            index: 0,
            length: 6,
        }))
        .into_result()?;
        println!("DFU status: {:x?}", status);
        self.packet_count += 1;
        self.get_dfu_status()?;

        // DFU_CMD_READ
        println!("Setting read command address...");
        block_on(self.device_handle.control_out(ControlOut {
            control_type: nusb::transfer::ControlType::Class,
            recipient: nusb::transfer::Recipient::Interface,
            request: 1,
            value: self.packet_count,
            index: 0,
            data: &[
                2, // DFU_CMD_READ
                0, // Reserved
                0xf0,
                0x03,                                // Start block number
                CONFIGURATION_SIZE.to_le_bytes()[0], // Image size
                CONFIGURATION_SIZE.to_le_bytes()[1],
                0,
                0,
            ],
        }))
        .into_result()?;
        self.packet_count += 1;
        self.get_dfu_status()?;

        // Disable the DFU header when reading back
        println!("Disabling read header...");
        block_on(self.device_handle.control_out(ControlOut {
            control_type: nusb::transfer::ControlType::Class,
            recipient: nusb::transfer::Recipient::Interface,
            request: 1,
            value: self.packet_count,
            index: 0,
            data: &[
                6, // DFU_CMD_BIN
                1, // Disable upload prefix
                0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
        }))
        .into_result()?;
        self.get_dfu_status()?;
        self.packet_count += 1;

        println!("Reading data...");
        let mut configuration = vec![];
        let mut offset = 0;
        while offset < 16384 {
            let bytes = block_on(self.device_handle.control_in(ControlIn {
                control_type: nusb::transfer::ControlType::Class,
                recipient: nusb::transfer::Recipient::Interface,
                request: 2,
                value: self.packet_count,
                index: offset,
                length: 1024,
            }))
            .into_result()?;
            self.packet_count += 1;
            configuration.extend_from_slice(&bytes);
            offset += 1024;
        }
        self.get_dfu_status()?;
        Ok(configuration)
    }

    fn write_configuration(
        &mut self,
        configuration: &[u8],
    ) -> Result<(), nusb::transfer::TransferError> {
        if configuration.len() != CONFIGURATION_SIZE as usize {
            panic!("Configuration length is unexpected");
        }

        // DFU_CMD_WRITE
        println!("Setting write command address...");
        block_on(self.device_handle.control_out(ControlOut {
            control_type: nusb::transfer::ControlType::Class,
            recipient: nusb::transfer::Recipient::Interface,
            request: 1,
            value: self.packet_count,
            index: 0,
            data: &[
                1, // DFU_CMD_WRITE
                0, // Reserved
                0xf0,
                0x03,                                // Start block number
                CONFIGURATION_SIZE.to_le_bytes()[0], // Image size
                CONFIGURATION_SIZE.to_le_bytes()[1],
                0,
                0,
            ],
        }))
        .into_result()?;
        self.packet_count += 1;
        self.get_dfu_status()?;

        for data in configuration.chunks(1024) {
            // Wait for the device to be ready to receive bytes
            while self.get_dfu_status()?[4] != 5 {}
            block_on(self.device_handle.control_out(ControlOut {
                control_type: nusb::transfer::ControlType::Class,
                recipient: nusb::transfer::Recipient::Interface,
                request: 1,
                value: self.packet_count,
                index: 0,
                data,
            }))
            .into_result()?;
            self.packet_count += 1;
        }

        // Finish the download
        while self.get_dfu_status()?[4] != 5 {}
        block_on(self.device_handle.control_out(ControlOut {
            control_type: nusb::transfer::ControlType::Class,
            recipient: nusb::transfer::Recipient::Interface,
            request: 1,
            value: self.packet_count,
            index: 0,
            data: &[],
        }))
        .into_result()?;
        self.packet_count += 1;

        while self.get_dfu_status()?[4] != 2 {}

        Ok(())
    }

    fn reset(mut self) -> Result<(), nusb::transfer::TransferError> {
        while self.get_dfu_status()?[4] != 2 {}
        // DFU_CMD_RESET
        block_on(self.device_handle.control_out(ControlOut {
            control_type: nusb::transfer::ControlType::Class,
            recipient: nusb::transfer::Recipient::Interface,
            request: 1,
            value: self.packet_count,
            index: 0,
            data: &[
                7, // DFU_CMD_RESET
                0x20, 0xdf, 0x00, 0x01, 0, 0, 0,
            ],
        }))
        .into_result()?;
        self.packet_count += 1;

        while self.get_dfu_status()?[4] != 2 {}

        Ok(())
    }
}

fn open_xds110() -> Result<Xds110UsbDevice, std::io::Error> {
    let devices = nusb::list_devices()?;
    let mut device_info = None;
    'next_device: for candidate_device in devices {
        for candidate_match in XDS110_USB_DEVICES {
            if candidate_device.vendor_id() == candidate_match.vid
                && candidate_device.product_id() == candidate_match.pid
            {
                if device_info.is_some() {
                    return Err(std::io::ErrorKind::TooManyLinks.into());
                }
                device_info = Some((
                    candidate_device,
                    candidate_match.epin,
                    candidate_match.epout,
                    candidate_match.interface,
                ));
                break 'next_device;
            }
        }
    }

    let Some((device, epin, epout, iface)) = device_info else {
        return Err(std::io::ErrorKind::NotFound.into());
    };

    let mut epout_found = false;
    let mut epin_found = false;

    let device_handle = device.open()?;

    let mut configs = device_handle.configurations();
    let Some(config) = configs.next() else {
        return Err(std::io::ErrorKind::NotFound.into());
    };
    let Some(interface) = config.interfaces().find(|x| x.interface_number() == iface) else {
        return Err(std::io::ErrorKind::NotFound.into());
    };

    for alt_setting in interface.alt_settings() {
        for endpoint in alt_setting.endpoints() {
            if endpoint.address() == epout {
                epout_found = true;
            } else if endpoint.address() == epin {
                epin_found = true;
            }
        }
    }

    if !epout_found || !epin_found {
        return Err(std::io::ErrorKind::NotFound.into());
    }

    let device_handle = device_handle.claim_interface(iface)?;

    Ok(Xds110UsbDevice {
        device_handle,
        epout,
        epin,
    })
}

fn open_dfu() -> Result<Xds110DfuDevice, std::io::Error> {
    let devices = nusb::list_devices()?;
    let mut device_info = None;
    'next_device: for candidate_device in devices {
        for candidate_match in XDS110_DFU_DEVICES {
            if candidate_device.vendor_id() == candidate_match.vid
                && candidate_device.product_id() == candidate_match.pid
            {
                if device_info.is_some() {
                    return Err(std::io::ErrorKind::TooManyLinks.into());
                }
                device_info = Some(candidate_device);
                break 'next_device;
            }
        }
    }

    let Some(device) = device_info else {
        eprintln!("Unable to find device");
        return Err(std::io::ErrorKind::NotFound.into());
    };

    let device_handle = device.open()?;

    // TODO: We may need to claim interface 0 on Windows, in which case this
    // struct will need to grow an `enum`.

    Ok(Xds110DfuDevice {
        device_handle,
        packet_count: 0,
    })
}

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let mut dfu = match open_dfu() {
        Ok(dfu) => dfu,
        Err(_) => {
            let xds110 = open_xds110()?;
            let version = xds110.firmware_version()?;

            if version < CMSIS_DAP_2_MINIMUM {
                let found = version.to_be_bytes();
                let minimum = CMSIS_DAP_2_MINIMUM.to_be_bytes();
                return Err(format!("CMSIS-DAP 2.0 is only supported on firmware versions >= {:02x}.{:02x}.{:02x}.{:02x} -- Your firmware is {:02x}.{:02x}.{:02x}.{:02x}",
            minimum[0], minimum[1], minimum[2], minimum[3],
            found[0], found[1], found[2], found[3]
        ).into());
            }

            xds110.reboot_to_dfu()?;
            // Wait for it to re-enumerate (TODO: Longer polling time?)
            std::thread::sleep(Duration::from_secs(1));
            open_dfu()?
        }
    };

    println!("Ensuring Tiva protocol...");
    dfu.ensure_binary_protocol()?;
    println!("Reading current configuration...");
    let mut configuration = dfu.read_configuration()?;

    if configuration[18..20] != [0x55, 0xaa] {
        println!(
            "Warning: Magic value not found! Expected [0x55, 0xaa], found {:02x?}",
            &configuration[18..20]
        );
        configuration[17] = 0;
        configuration[18] = 0x55;
        configuration[19] = 0xaa;
    }
    let current_mode = u16::from_le_bytes(configuration[16..18].try_into().unwrap());
    println!("Current mode: {:02x?}", current_mode);

    if current_mode == 4 {
        println!("Device was already in mode 4");
        return Ok(());
    }
    println!("Updating device from mode {} to mode 4", current_mode);
    configuration[16] = 4;

    dfu.write_configuration(&configuration)?;
    println!("Resetting into normal mode");
    dfu.reset()?;

    Ok(())
}
