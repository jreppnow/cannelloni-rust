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

use crate::async_can::AsyncCanSocket;
use smol::net::{UdpSocket, SocketAddr};

async fn send(can_socket: &AsyncCanSocket, udp_socket: &UdpSocket) {
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

/// Listen for packets on a UDP socket, unwrap them and transfer them onto a can socket..
///
/// # Arguments
///
/// * `can_socket`: The CAN socket (destination).
/// * `udp_socket`:  The UDP socket (source).
/// * `local_address`: The local address of this applications. If the sender address of a packet is the same as this, it will be ignored. Needed for multicast.
///
/// returns: ()
async fn receive(can_socket: &AsyncCanSocket, udp_socket: &UdpSocket, local_address: SocketAddr /* TODO: Make this an Option? Unfortunately does not play nice with match or if let.. */)  {
    let mut buffer = vec![0u8; 1024];
    while let Ok((n, peer)) = udp_socket.recv_from(&mut buffer).await {
        if peer != local_address {
            if let Some(decoder) = proto::MessageReader::try_read(&buffer[..n]) {
                for frame in decoder {
                    let _ = can_socket.write_frame(&frame).await;
                }
            }
        }
    }
}

fn main() {
    use futures::prelude::*;

    smol::block_on(async {
        let udp_socket = UdpSocket::bind("127.0.0.1:5678").await.unwrap();
        let can_socket: AsyncCanSocket = socketcan::CANSocket::open("vcan1").unwrap().into();

        futures::select! {
            _ = send(&can_socket, &udp_socket).fuse() => (),
            _ = receive(&can_socket, &udp_socket, udp_socket.local_addr().expect("Failed to get local addr!")).fuse() => (),
        }
    });
}
