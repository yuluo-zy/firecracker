// Copyright (C) 2019-2023 Alibaba Cloud. All rights reserved.
// Copyright (C) 2019-2023 Ant Group. All rights reserved.
// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.

use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;
use event_manager::SubscriberId;
use log::trace;
use vm_memory::{GuestAddressSpace, GuestMemoryRegion};
use crate::devices::virtio::net::{gen, NetError, Tap, VirtioDeviceInfo};
use vhost::vhost_kern::net::Net as VhostNet;
use utils::eventfd::EventFd;
use utils::net::mac::MacAddr;
use crate::devices::virtio::device::{DeviceState, IrqTrigger};
use crate::devices::virtio::gen::virtio_net::{VIRTIO_F_NOTIFY_ON_EMPTY, VIRTIO_F_VERSION_1, VIRTIO_NET_F_CSUM, VIRTIO_NET_F_CTRL_VQ, VIRTIO_NET_F_GUEST_CSUM, VIRTIO_NET_F_GUEST_TSO4, VIRTIO_NET_F_GUEST_UFO, VIRTIO_NET_F_HOST_TSO4, VIRTIO_NET_F_HOST_UFO, VIRTIO_NET_F_MAC, VIRTIO_NET_F_MQ, VIRTIO_NET_F_MRG_RXBUF, VIRTIO_NET_F_STATUS, VIRTIO_RING_F_INDIRECT_DESC};
use crate::devices::virtio::gen::virtio_ring::VIRTIO_RING_F_EVENT_IDX;
use crate::devices::virtio::net::device::{ConfigSpace, vnet_hdr_len};
use crate::devices::virtio::net::vhost::VhostNetError;
use crate::devices::virtio::queue::Queue;
use crate::rate_limiter::RateLimiter;

const NET_DRIVER_NAME: &str = "vhost-net";
// Epoll token for control queue
const CTRL_SLOT: u32 = 0;
// Control queue size
const CTRL_QUEUE_SIZE: u16 = 64;

pub const DEFAULT_MTU: u16 = 1500;

/// Ensure that the tap interface has the correct flags and sets the
/// offload and VNET header size to the appropriate values.
fn validate_and_configure_tap(tap: &Tap, vq_pairs: usize) -> Result<(), VhostNetError> {
    // Check if there are missing flags。
    let flags = tap.if_flags();
    let mut required_flags = vec![
        (gen::IFF_TAP, "IFF_TAP"),
        (gen::IFF_NO_PI, "IFF_NO_PI"),
        (gen::IFF_VNET_HDR, "IFF_VNET_HDR"),
    ];
    if vq_pairs > 1 {
        required_flags.push((gen::IFF_MULTI_QUEUE, "IFF_MULTI_QUEUE"));
    }
    let missing_flags = required_flags
        .iter()
        .filter_map(
            |(value, name)| {
                if value & flags == 0 {
                    Some(name)
                } else {
                    None
                }
            },
        )
        .collect::<Vec<_>>();

    if !missing_flags.is_empty() {
        return Err(VhostNetError::MissingFlags(
            missing_flags
                .into_iter()
                .map(|flag| *flag)
                .collect::<Vec<&str>>()
                .join(", ")));
    }

    tap.set_offload(gen::TUN_F_CSUM | gen::TUN_F_UFO | gen::TUN_F_TSO4 | gen::TUN_F_TSO6)
        .map_err(VhostNetError::TapSetOffload)?;
    let vnet_hdr_size = vnet_hdr_len() as i32;
    tap.set_vnet_hdr_size(vnet_hdr_size)
        .map_err(VhostNetError::TapSetVnetHdrSize)?;
    Ok(())
}


/// Vhost-net device implementation
pub struct Net
{
    taps: Vec<Tap>,
    pub(crate) id: String,

    pub(crate) avail_features: u64, // 表示网络设备支持的可用功能，是一个位掩码，编码了设备支持的所有特性。
    pub(crate) acked_features: u64, // 表示已确认的功能集，是一个位掩码，编码了设备驱动程序已确认并使用的特性。

    pub(crate) queues: Vec<Queue>,
    pub(crate) queue_evts: Vec<EventFd>,

    pub(crate) rx_rate_limiter: RateLimiter,
    pub(crate) tx_rate_limiter: RateLimiter,

    pub(crate) irq_trigger: IrqTrigger,

    pub(crate) config_space: ConfigSpace,
    pub(crate) guest_mac: Option<MacAddr>,

    pub(crate) device_state: DeviceState,
    pub(crate) activate_evt: EventFd,

}

impl Net {
    /// Create a new vhost-net device with a given tap interface.
    pub fn new_with_tap(
        id: String,
        tap: Tap,
        guest_mac: Option<MacAddr>,
        queue_sizes: Arc<Vec<u16>>,
        rx_rate_limiter: RateLimiter,
        tx_rate_limiter: RateLimiter,
    ) -> Result<Self, VhostNetError> {
        trace!(target: "vhost-net", "{}: Net::new_with_tap()", NET_DRIVER_NAME);

        let vq_pairs = queue_sizes.len() / 2;

        let taps = tap.into_mq_taps(vq_pairs).map_err(VhostNetError::TapOpen)?;
        for tap in taps.iter() {
            validate_and_configure_tap(tap, vq_pairs)?;
        }

        let mut avail_features = 1u64 << VIRTIO_NET_F_GUEST_CSUM
            | 1u64 << VIRTIO_NET_F_CSUM
            | 1u64 << VIRTIO_NET_F_GUEST_TSO4
            | 1u64 << VIRTIO_NET_F_GUEST_UFO
            | 1u64 << VIRTIO_NET_F_HOST_TSO4
            | 1u64 << VIRTIO_NET_F_HOST_UFO
            | 1u64 << VIRTIO_NET_F_MRG_RXBUF
            | 1u64 << VIRTIO_RING_F_INDIRECT_DESC
            | 1u64 << VIRTIO_RING_F_EVENT_IDX
            | 1u64 << VIRTIO_F_NOTIFY_ON_EMPTY
            | 1u64 << VIRTIO_F_VERSION_1;

        if vq_pairs > 1 {
            avail_features |= (1 << VIRTIO_NET_F_MQ | 1 << VIRTIO_NET_F_CTRL_VQ) as u64;
        }

        let mut config_space = ConfigSpace::default();
        config_space.setup_config_space(
            NET_DRIVER_NAME,
            guest_mac,
            &mut avail_features,
            vq_pairs as u16,
            DEFAULT_MTU,
        );
        let mut queue_evts = Vec::new();
        let mut queues = Vec::new();
        for size in queue_sizes {
            queue_evts.push(EventFd::new(libc::EFD_NONBLOCK).map_err(VhostNetError::EventFd)?);
            queues.push(Queue::new(size)); // 两个256
        }

        Ok(Net {
            taps,
            id: id.clone(),
            avail_features,
            acked_features: 0u64,
            queues,
            queue_evts,
            rx_rate_limiter,
            tx_rate_limiter,
            irq_trigger:  IrqTrigger::new().map_err(VhostNetError::EventFd)?,
            config_space,
            guest_mac,
            device_state: DeviceState::Inactive,
            activate_evt: EventFd::new(libc::EFD_NONBLOCK).map_err(VhostNetError::EventFd)?,
        })
    }

    /// Create a vhost network with the Tap name
    pub fn new(
        id: String,
        tap_if_name: &str,
        guest_mac: Option<MacAddr>,
        queue_sizes: Arc<Vec<u16>>,
        rx_rate_limiter: RateLimiter,
        tx_rate_limiter: RateLimiter,
    ) -> Result<Self, VhostNetError> {
        let vq_pairs = queue_sizes.len() / 2;

        // Open a TAP interface
        let tap = Tap::open_named(&tap_if_name, vq_pairs > 1)
            .map_err(VhostNetError::TapOpen)?;
        tap.set_offload(gen::TUN_F_CSUM | gen::TUN_F_UFO | gen::TUN_F_TSO4 | gen::TUN_F_TSO6)
            .map_err(VhostNetError::TapSetOffload)?;
        // 获取虚拟网络头部长度：
        let vnet_hdr_size = i32::try_from(vnet_hdr_len()).unwrap();
        tap.set_vnet_hdr_size(vnet_hdr_size)
            .map_err(VhostNetError::TapSetVnetHdrSize)?;
        Self::new_with_tap(id, tap, guest_mac, queue_sizes, rx_rate_limiter, tx_rate_limiter)
    }
}
