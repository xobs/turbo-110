use async_io::{Timer, block_on};
use futures_lite::FutureExt;
use nusb::{Interface, transfer::RequestBuffer};
use std::{io, time::Duration};

const MAX_PACKET_LENGTH: usize = 64;

pub trait InterfaceExt {
    fn read_bulk(&self, endpoint: u8, buf: &mut [u8], timeout: Duration) -> io::Result<usize>;
    fn write_bulk(&self, endpoint: u8, buf: &[u8], timeout: Duration) -> io::Result<usize>;
}

impl InterfaceExt for Interface {
    fn write_bulk(&self, endpoint: u8, buf: &[u8], timeout: Duration) -> io::Result<usize> {
        let fut = async {
            let comp = self.bulk_out(endpoint, buf.to_vec()).await;
            comp.status.map_err(io::Error::other)?;

            let n = comp.data.actual_length();
            Ok(n)
        };

        block_on(fut.or(async {
            Timer::after(timeout).await;
            Err(std::io::ErrorKind::TimedOut.into())
        }))
    }

    fn read_bulk(&self, endpoint: u8, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        let fut = async {
            if buf.len() > 64 {
                let mut queue = self.bulk_in_queue(endpoint);
                // Add one byte for the ZLP
                let transfer_size = MAX_PACKET_LENGTH;
                let n_transfers = (buf.len() + 1) / transfer_size;
                while queue.pending() < n_transfers {
                    queue.submit(RequestBuffer::new(transfer_size));
                }

                let mut offset = 0;
                loop {
                    let comp = queue.next_complete().await;
                    comp.status.map_err(io::Error::other)?;

                    let n = comp.data.len();
                    buf[offset..offset + n].copy_from_slice(&comp.data);
                    offset += n;
                    // Finish on ZLP, a non-full buffer, or if we've filled the buffer
                    if n == 0 || n != transfer_size || offset == buf.len() {
                        break;
                    }
                    queue.submit(RequestBuffer::reuse(comp.data, transfer_size));
                }
                Ok(offset)
            } else {
                let comp = self.bulk_in(endpoint, RequestBuffer::new(buf.len())).await;
                comp.status.map_err(io::Error::other)?;

                let n = comp.data.len();
                buf[..n].copy_from_slice(&comp.data);
                Ok(n)
            }
        };

        block_on(fut.or(async {
            Timer::after(timeout).await;
            Err(std::io::ErrorKind::TimedOut.into())
        }))
    }
}
