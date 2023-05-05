/*
 *  Copyright (c) 2022 Janosch Reppnow.
 *
 *  This program is free software; you can redistribute it and/or
 *  modify it under the terms of the GNU Lesser General Public
 *  License as published by the Free Software Foundation; either
 *  version 3 of the License, or (at your option) any later version.
 *
 *  This program is distributed in the hope that it will be useful,
 *  but WITHOUT ANY WARRANTY; without even the implied warranty of
 *  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 *  Lesser General Public License for more details.
 *
 *  You should have received a copy of the GNU Lesser General Public License
 *  along with this program; if not, write to the Free Software Foundation,
 *  Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301, USA.
 */

use core::ffi::c_size_t;
use std::ffi::c_void;
use std::mem::{MaybeUninit, size_of};
use std::os::unix::io::AsRawFd;

use async_io::Async;
use libc::can_frame;
pub use socketcan::CanFrame;
use socketcan::CanSocket;
use socketcan::frame::AsPtr;

pub struct AsyncCanSocket {
    watcher: Async<CanSocket>,
}

impl From<CanSocket> for AsyncCanSocket {
    fn from(socket: CanSocket) -> Self {
        Self {
            watcher: Async::new(socket).unwrap(),
        }
    }
}

impl AsyncCanSocket {
    pub async fn read_frame(&self) -> std::io::Result<CanFrame> {
        let mut frame = MaybeUninit::<can_frame>::uninit();
        self.watcher
            .read_with(|fd| {
                let ret = unsafe {
                    libc::read(
                        fd.as_raw_fd(),
                        frame.as_mut_ptr() as *mut c_void,
                        size_of::<can_frame>() as c_size_t,
                    )
                };
                if ret > 0 {
                    Ok(ret)
                } else {
                    Err(std::io::Error::last_os_error())
                }
            })
            .await?;

        // Safety: Return value was okay and we trust the c library to have properly
        // filled the value.
        let frame = unsafe { frame.assume_init() };
        Ok(frame.into())
    }

    pub async fn write_frame(&self, frame: &CanFrame) -> std::io::Result<()> {
        self.watcher
            .write_with(|fd| {
                let ret = unsafe {
                    libc::write(
                        fd.as_raw_fd(),
                        frame.as_ptr() as *const c_void,
                        size_of::<can_frame>() as c_size_t,
                    )
                };
                if ret > 0 {
                    Ok(ret)
                } else {
                    Err(std::io::Error::last_os_error())
                }
            })
            .await?;

        Ok(())
    }
}

#[cfg(feature = "vcan_testing")]
#[cfg(test)]
mod tests {
    use std::future::join;

    use socketcan::{EmbeddedFrame, Id, Socket, StandardId};

    use crate::async_can::AsyncCanSocket;

    #[test]
    fn async_read_and_write() {
        use futures::{executor, join};

        let writer: AsyncCanSocket = socketcan::CanSocket::open("vcan").unwrap().into();
        let reader: AsyncCanSocket = socketcan::CanSocket::open("vcan").unwrap().into();

        let frame =
            socketcan::CanFrame::new(Id::Standard(StandardId::new(0x14).unwrap()), &[1, 3, 3, 7])
                .unwrap();

        executor::block_on(async {
            let (write_result, read_result) =
                join!(writer.write_frame(&frame), reader.read_frame());

            assert!(write_result.is_ok());
            assert_eq!(frame.data(), read_result.unwrap().data());
        });
    }
}
