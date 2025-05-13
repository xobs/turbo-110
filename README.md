# Turbo mode for XDS110

Convert your XDS110 probe to use CMSIS-DAP 2.0 mode.

## Background

TI produces the XDS110 debug probe. By default, this probe exposes two different kinds of interfaces:

1. A "TI Proprietary" interface that uses an undocumented protocol
2. An industry-standard "CMSIS-DAP" protocol used by many debuggers

By default, the CMSIS-DAP protocol uses version 1.0 of the spec, which runs via HID descriptors. HID has a maximum poll rate of 1 ms, meaning that when you add in the outgoing and incoming packets you have a maximum packet rate of one packet every two milliseconds.

TI has released "Alternate Mode 4" which allows you to upgrade this to a "CMSIS-DAP 2.0" protocol, however it can be hard to find and requires you to use their `xdsdfu` tool.

This reimplements this tool in Rust, and allows you to switch to the faster mode more easily.

## References

This uses the [Tiva USB DFU Class](https://www.ti.com/lit/an/spma054/spma054.pdf) reference, as well as the standard [USB DFU](https://www.usb.org/sites/default/files/DFU_1.1.pdf) specification.
