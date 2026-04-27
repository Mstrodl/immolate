use std::time::Duration;

use anyhow::Context;
use hidapi::{DeviceInfo, HidApi, HidDevice, MAX_REPORT_DESCRIPTOR_SIZE};
use hidparser::{
    ReportDescriptor, ReportField, parse_report_descriptor, report_data_types::ReportId,
};
use nusb::MaybeFuture;

const CYBERPOWER_VID: u16 = 0x0764;
const CYBERPOWER_UPS_PID: u16 = 0x0601;

// Source: https://www.usb.org/sites/default/files/pdcv10_0.pdf
const OUTPUT_USAGE: u16 = 0x001c;
const OUTPUT_USAGE_PAGE: u16 = 0x0084;
const DELAY_BEFORE_STARTUP: u16 = 0x56;
const DELAY_BEFORE_SHUTDOWN: u16 = 0x57;

// This is not the right way to do this...
fn find_report_for(descriptor: &ReportDescriptor, usage: u16) -> Option<ReportId> {
    descriptor.features.iter().find_map(|feature| {
        if feature.fields.len() == 1
            && matches!(feature.fields[0], ReportField::Variable(ref variable) if
                variable.usage.id() == usage && variable.bits == (0..16))
        {
            feature.report_id
        } else {
            None
        }
    })
}

#[derive(Debug)]
struct UpsDescriptor<'a> {
    dev: &'a HidDevice,
    delay_before_shutdown: ReportId,
    delay_before_startup: ReportId,
}

impl<'a> UpsDescriptor<'a> {
    fn new(dev: &'a HidDevice) -> anyhow::Result<Self> {
        let mut descriptor_buf = [0u8; MAX_REPORT_DESCRIPTOR_SIZE];
        let descriptor_length = dev.get_report_descriptor(&mut descriptor_buf)?;
        let descriptor_buf = &descriptor_buf[..descriptor_length];

        Self::new_from_report_descriptor(dev, descriptor_buf)
            .with_context(|| format!("Parsing descriptor: {descriptor_buf:?}"))
    }

    fn new_from_report_descriptor(
        dev: &'a HidDevice,
        descriptor_buf: &[u8],
    ) -> anyhow::Result<Self> {
        let descriptor = parse_report_descriptor(descriptor_buf)
            .map_err(|err| anyhow::anyhow!("Couldn't parse report descriptor: {err:?}"))?;
        Ok(UpsDescriptor {
            dev,
            delay_before_shutdown: find_report_for(&descriptor, DELAY_BEFORE_SHUTDOWN)
                .context("DelayBeforeShutdown")?,
            delay_before_startup: find_report_for(&descriptor, DELAY_BEFORE_STARTUP)
                .context("DelayBeforeStartup")?,
        })
    }

    fn set(&self, report_id: ReportId, delay: Duration) -> anyhow::Result<()> {
        let delay = u16::to_le_bytes((delay.as_secs()).try_into().unwrap_or(u16::MAX));
        let report_id = u32::from(report_id) as u8;
        self.dev
            .send_feature_report(&[report_id, delay[0], delay[1]])
            .with_context(|| format!("Setting report 0x{report_id:x} to (hex) {delay:x?}"))?;
        Ok(())
    }

    pub fn set_delay_before_shutdown(&self, delay: Duration) -> anyhow::Result<()> {
        self.set(self.delay_before_shutdown, delay)
    }

    pub fn set_delay_before_startup(&self, delay: Duration) -> anyhow::Result<()> {
        self.set(self.delay_before_startup, delay)
    }
}

fn reboot_ups(api: &HidApi, ups_info: &DeviceInfo) -> anyhow::Result<()> {
    let dev = ups_info.open_device(api)?;
    println!("Shutting off UPS {ups_info:?}...");

    let ups = UpsDescriptor::new(&dev)?;
    println!("Which has these descriptors: {ups:?}...");

    const DELAY: Duration = Duration::from_secs(0);
    ups.set_delay_before_startup(DELAY)?;
    std::thread::sleep(Duration::from_micros(125_000));
    ups.set_delay_before_shutdown(DELAY)?;

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let candidate_upses = nusb::list_devices()
        .wait()?
        .filter(|dev| dev.vendor_id() == CYBERPOWER_VID && dev.product_id() == CYBERPOWER_UPS_PID);
    for ups_info in candidate_upses {
        let dev = ups_info.open().wait()?;
        for interface in ups_info.interfaces() {
            if let Err(err) = dev.attach_kernel_driver(interface.interface_number())
                && err.kind() != nusb::ErrorKind::Busy
            {
                eprintln!(
                    "Couldn't reattach kernel driver for interface {interface:?} of {ups_info:?}! Hopefully the hidraw device is still there? {err}"
                );
            }
        }
    }

    let api = HidApi::new()?;
    let upses = api
        .device_list()
        .inspect(|info| {
            println!(
                "Info: {info:?}, usage: 0x{:x}, usage page: 0x{:x}",
                info.usage(),
                info.usage_page()
            );
        })
        .filter(|info| {
            info.vendor_id() == CYBERPOWER_VID
                && info.product_id() == CYBERPOWER_UPS_PID
                && (cfg!(feature = "hidraw") || info.usage() == OUTPUT_USAGE)
                && info.usage_page() == OUTPUT_USAGE_PAGE
        });

    let mut found_one = false;
    let mut errored = false;
    for ups_info in upses {
        found_one = true;

        if let Err(err) = reboot_ups(&api, ups_info) {
            eprintln!("Failed to reboot UPS {ups_info:?}: {err}");
            errored = true;
        }
    }

    if !found_one {
        return Err(anyhow::anyhow!("Didn't find any matching UPSes!"));
    }
    if errored {
        return Err(anyhow::anyhow!(
            "One or more UPSes did not successfully reboot"
        ));
    }

    println!("Bye...");
    Ok(())
}
