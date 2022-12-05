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
use async_io::Timer;
use smol::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use futures::prelude::*;

async fn send(can_socket: &AsyncCanSocket, udp_socket: &UdpSocket, force_after: Duration) {
    let mut encoder = MessageSerializer::new();

    let mut count = 0usize;

    let mut timer: Option<Fuse<Timer>> = None;

    loop {
        timer = match timer {
            None => {
                if let Ok(frame) = can_socket.read_frame().await {
                    count += 1;
                    encoder.push_frame(frame);
                    if count == 10 {
                        count = 0;
                        send_frame(udp_socket, &mut encoder).await;
                        None
                    } else {
                        Some(futures::FutureExt::fuse(Timer::after(force_after)))
                    }
                } else {
                    break;
                }
            }
            Some(mut timer) => {
                select! {
                    _ = &mut timer => {
                        send_frame(udp_socket, &mut encoder).await;
                        None
                    }
                    frame = can_socket.read_frame().fuse() => {
                        if let Ok(frame) = frame {
                            count += 1;
                            encoder.push_frame(frame);
                            if count == 10 {
                                count = 0;
                                send_frame(udp_socket, &mut encoder).await;
                                None
                            } else {
                                Some(timer)
                            }
                        } else {
                            Some(timer)
                        }
                    }
                }
            }
        }
    }
}

async fn send_frame(udp_socket: &UdpSocket, encoder: &mut MessageSerializer) {
    use crate::proto::SerializeInto;
    let mut serialized = encoder.serialize();
    udp_socket.send(serialized.make_contiguous()).await.unwrap();
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
async fn receive(
    can_socket: &AsyncCanSocket,
    udp_socket: &UdpSocket,
    local_address: SocketAddr, /* TODO: Make this an Option? Unfortunately does not play nice with match or if let.. */
) {
    let mut buffer = [0u8; 1024];
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

use crate::future::Fuse;
use crate::proto::MessageSerializer;
use clap::Parser;
use futures::select;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Local address to bind to.
    #[arg(long, short)]
    bind: String,

    /// Remote address to connect to.
    #[arg(long, short)]
    remote: String,

    /// CAN interface name to forward frames to and from.
    #[arg(long, short)]
    can: String,

    /// After how many milliseconds a packet must be sent, regardless of how many frames it contains.
    #[arg(long, short, default_value_t = 50)]
    force_after_ms: usize,
}

fn main() {
    let args = Args::parse();

    smol::block_on(async {
        let remote_address: SocketAddr = args
            .remote
            .parse()
            .expect("Failed to parse remote address!");
        let local_address: SocketAddr = args.bind.parse().expect("Failed to parse local address.");

        let udp_socket = UdpSocket::bind(local_address)
            .await
            .expect("Failed to bind to local address.");
        udp_socket
            .connect(remote_address)
            .await
            .expect("Failed to connect to remote address");

        let can_socket: AsyncCanSocket = socketcan::CANSocket::open(&args.can).unwrap().into();

        select! {
            _ = send(&can_socket, &udp_socket, Duration::from_millis(args.force_after_ms as u64)).fuse() => (),
            _ = receive(&can_socket, &udp_socket, udp_socket.local_addr().expect("Failed to get local addr!")).fuse() => (),
        }
    });
}
