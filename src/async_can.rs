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
use std::mem::size_of;
use std::os::unix::io::AsRawFd;

use async_io::Async;
pub use socketcan::{CANFrame, CANSocket, CANSocketOpenError};

struct AsyncCanSocket {
    watcher: Async<CANSocket>,
}

impl From<CANSocket> for AsyncCanSocket {
    fn from(socket: CANSocket) -> Self {
        Self {
            watcher: Async::new(socket).unwrap(),
        }
    }
}

impl AsyncCanSocket {
    async fn read_frame(&self) -> std::io::Result<CANFrame> {
        let mut frame = CANFrame::new(0, &[0; 8], false, false).unwrap();
        self.watcher.read_with(|fd| {
            let ret = unsafe {
                libc::read(fd.as_raw_fd(), &mut frame as *mut CANFrame as *mut c_void, size_of::<CANFrame>() as c_size_t)
            };
            if ret > 0 {
                Ok(ret)
            } else {
                Err(std::io::Error::last_os_error())
            }
        }).await?;

        Ok(frame)
    }

    async fn write_frame(&self, frame: &CANFrame) -> std::io::Result<()> {
        self.watcher.write_with(|fd| {
            let ret = unsafe {
                libc::write(fd.as_raw_fd(), frame as *const CANFrame as *const c_void, size_of::<CANFrame>() as c_size_t)
            };
            if ret > 0 {
                Ok(ret)
            } else {
                Err(std::io::Error::last_os_error())
            }
        }).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::future::join;

    use crate::async_can::AsyncCanSocket;

    #[test]
    fn async_read_and_write() {
        use futures::{prelude::*, join, executor};

        let writer: AsyncCanSocket = socketcan::CANSocket::open("vcan").unwrap().into();
        let reader: AsyncCanSocket = socketcan::CANSocket::open("vcan").unwrap().into();

        let frame = socketcan::CANFrame::new(13, &[1, 3, 3, 7], false, false).unwrap();


        executor::block_on(async {
            let (write_result, read_result) = join!(writer.write_frame(&frame), reader.read_frame());

            assert!(write_result.is_ok());
            assert!(frame.data() == read_result.unwrap().data());
        });
    }
}