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

#![feature(c_size_t)]
#![feature(future_join)] // for tests..

mod async_can;
mod proto;

async fn send() {
    let udp_socket = async_std::net::UdpSocket::bind("127.0.0.1:5678").await.unwrap();
    let can_socket: async_can::AsyncCanSocket = socketcan::CANSocket::open("vcan0").unwrap().into();

    let mut encoder = proto::MessageSerializer::new();

    let mut count = 0usize;
    while let Ok(frame) = can_socket.read_frame().await {
        count += 1;
        encoder.push_frame(frame);
        if count == 10 {
            count = 0;
            use crate::proto::SerializeInto;
            let mut serialized = encoder.serialize();
            udp_socket.send_to(serialized.make_contiguous(), "127.0.0.2:5678").await.unwrap();
        }
    }
}

async fn receive() {
    let udp_socket = async_std::net::UdpSocket::bind("127.0.0.2:5678").await.unwrap();
    let can_socket: async_can::AsyncCanSocket = socketcan::CANSocket::open("vcan1").unwrap().into();

    let mut buffer = vec![0u8; 1024];
    while let Ok((n, _)) = udp_socket.recv_from(&mut buffer).await {
        if let Some(decoder) = proto::MessageReader::try_read(&buffer[..n]) {
            for frame in decoder {
                let _ = can_socket.write_frame(&frame).await;
            }
        }
    }
}

fn main() {
    use futures::prelude::*;

    async_std::task::block_on(async {
        futures::select! {
            _ = send().fuse() => (),
            _ = receive().fuse() => (),
        }
    });
}
