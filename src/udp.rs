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

use anyhow::Context;
use async_io::Timer;
use futures::future::Fuse;
use futures::{select, FutureExt};
use smol::net::UdpSocket;
use std::net::SocketAddr;
use std::time::Duration;

use crate::async_can::AsyncCanSocket;
use crate::proto;
use crate::proto::{MessageSerializer, SerializeInto};

const UDP_NO_FRAGMENT_MAX_PAYLOAD_SIZE: usize = 508;

pub async fn send(
    can_socket: &AsyncCanSocket,
    udp_socket: &UdpSocket,
    force_after: Duration,
    target: &SocketAddr,
) -> anyhow::Result<()> {
    let mut encoder = MessageSerializer::new();
    let mut timer: Option<Fuse<Timer>> = None;

    loop {
        timer = match timer {
            None => {
                let frame = can_socket
                    .read_frame()
                    .await
                    .context("Trying to read CAN frame..")?;
                if encoder.encoded_size() + frame.encoded_size() > UDP_NO_FRAGMENT_MAX_PAYLOAD_SIZE
                {
                    send_frame(udp_socket, &mut encoder, target)
                        .await
                        .context("Sending frame bundle after buffer size limit was reached..")?;
                }
                encoder.push_frame(frame);
                Some(Timer::after(force_after).fuse())
            }
            Some(mut timer) => {
                select! {
                    _ = &mut timer => {
                        send_frame(udp_socket, &mut encoder, target).await.context("Sending frame bundle after timer has expired..")?;
                        None
                    }
                    frame = can_socket.read_frame().fuse() => {
                        let frame = frame.context("Asynchronously reading frame from CAN socket..")?;
                        if encoder.encoded_size() + frame.encoded_size() > UDP_NO_FRAGMENT_MAX_PAYLOAD_SIZE {
                            send_frame(udp_socket, &mut encoder, target).await.context("Sending frame bundle after buffer size limit was reached..")?;
                            encoder.push_frame(frame);
                            // new timer, since we have sent the last packet..
                            Some(Timer::after(force_after).fuse())
                        } else {
                            encoder.push_frame(frame);
                            Some(timer)
                        }
                    }
                }
            }
        }
    }
}

async fn send_frame(
    udp_socket: &UdpSocket,
    encoder: &mut MessageSerializer,
    target: &SocketAddr,
) -> anyhow::Result<()> {
    let mut serialized = encoder.serialize();
    udp_socket
        .send_to(serialized.make_contiguous(), target)
        .await
        .map(|_| ())
        .context("Sending frame bundle via UDP..")
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
pub async fn receive(
    can_socket: &AsyncCanSocket,
    udp_socket: &UdpSocket,
    local_address: SocketAddr,
) -> anyhow::Result<()> {
    let mut buffer = [0u8; UDP_NO_FRAGMENT_MAX_PAYLOAD_SIZE];
    loop {
        let (n, peer) = udp_socket
            .recv_from(&mut buffer)
            .await
            .context("Failed to read from UDP socket!")?;
        if peer != local_address {
            if let Some(decoder) = proto::MessageReader::try_read(&buffer[..n]) {
                for frame in decoder {
                    let _ = can_socket.write_frame(&frame).await;
                }
            }
        }
    }
}

pub async fn create_udp_sockets(
    local: &SocketAddr,
    remote: &SocketAddr,
) -> anyhow::Result<(UdpSocket, UdpSocket)> {
    match (remote, local) {
        (SocketAddr::V4(remote), SocketAddr::V4(local)) if remote.ip().is_multicast() => {
            let udp_receiver = UdpSocket::bind(remote)
                .await
                .context(format!("Binding to multicast addr: {remote}.."))?;
            udp_receiver
                .join_multicast_v4(*remote.ip(), *local.ip())
                .context(format!("Joining IPv4 multicast group: {}..", remote.ip()))?;

            let udp_sender = UdpSocket::bind(local)
                .await
                .context(format!("Binding sender socket to local address: {local}.."))?;
            Ok((udp_sender, udp_receiver))
        }
        (SocketAddr::V6(remote), SocketAddr::V6(local)) if remote.ip().is_multicast() => {
            let udp_receiver = UdpSocket::bind(remote)
                .await
                .context(format!("Binding to multicast addr: {remote}.."))?;
            udp_receiver
                .join_multicast_v6(remote.ip(), remote.scope_id())
                .context(format!("Joining IPv6 multicast group: {}..", remote.ip()))?;

            let udp_sender = UdpSocket::bind(local)
                .await
                .context(format!("Binding sender socket to local address: {local}.."))?;
            Ok((udp_sender, udp_receiver))
        }
        (SocketAddr::V4(_), SocketAddr::V4(_)) | (SocketAddr::V6(_), SocketAddr::V6(_)) => {
            let udp_socket = bind_simple(local, remote)
                .await
                .context("Binding unicast socket..")?;
            Ok((udp_socket.clone(), udp_socket))
        }
        _ => anyhow::bail!("Must specify EITHER IPv4 or IPv6 for both bind and connect addresses!"),
    }
}

async fn bind_simple(
    local_address: &SocketAddr,
    remote_address: &SocketAddr,
) -> anyhow::Result<UdpSocket> {
    let udp_socket = UdpSocket::bind(local_address)
        .await
        .context(format!("Binding to local address: {local_address}.."))?;
    udp_socket
        .connect(remote_address)
        .await
        .context(format!("Connecting to remote address: {remote_address}.."))?;
    Ok(udp_socket)
}
