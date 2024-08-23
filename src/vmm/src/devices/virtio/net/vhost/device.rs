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
use std::sync::atomic::AtomicU32;
use event_manager::SubscriberId;
use log::trace;
use vm_memory::{GuestAddressSpace, GuestMemoryRegion};
use crate::devices::virtio::net::{gen, NetError, Tap, VirtioDeviceInfo};
use vhost::vhost_kern::net::Net as VhostNet;
use vhost::VhostBackend;
use utils::eventfd::EventFd;
use utils::net::mac::MacAddr;
use crate::devices::virtio::{ActivateError, TYPE_NET};
use crate::devices::virtio::device::{DeviceState, IrqTrigger, VirtioDevice};
use crate::devices::virtio::gen::virtio_net::{VIRTIO_F_NOTIFY_ON_EMPTY, VIRTIO_F_VERSION_1, VIRTIO_NET_F_CSUM, VIRTIO_NET_F_CTRL_VQ, VIRTIO_NET_F_GUEST_CSUM, VIRTIO_NET_F_GUEST_ECN, VIRTIO_NET_F_GUEST_TSO4, VIRTIO_NET_F_GUEST_TSO6, VIRTIO_NET_F_GUEST_UFO, VIRTIO_NET_F_HOST_TSO4, VIRTIO_NET_F_HOST_UFO, VIRTIO_NET_F_MAC, VIRTIO_NET_F_MQ, VIRTIO_NET_F_MRG_RXBUF, VIRTIO_NET_F_STATUS, VIRTIO_RING_F_INDIRECT_DESC};
use crate::devices::virtio::gen::virtio_ring::VIRTIO_RING_F_EVENT_IDX;
use crate::devices::virtio::net::device::{ConfigSpace, vnet_hdr_len};
use crate::devices::virtio::net::vhost::VhostNetError;
use crate::devices::virtio::queue::Queue;
use crate::rate_limiter::RateLimiter;
use crate::vstate::memory::GuestMemoryMmap;

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

    handles: Vec<VhostNet<GuestMemoryMmap>>,
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
            handles: vec![],
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

    fn do_device_activate(&mut self, mem: GuestMemoryMmap, vq_pairs: usize) -> Result<(), VhostNetError> {
        if self.handles.is_empty() {
            for _ in 0..vq_pairs {
                self.handles.push(VhostNet::<GuestMemoryMmap>::new(mem.clone())
                                      .map_err(|error| VhostNetError::VhostError(error))?);
            }
        }
        self.setup_vhost_backend(mem, vq_pairs)?;
        Ok(())
    }

    fn setup_vhost_backend(&mut self, mem: GuestMemoryMmap,vq_pairs: usize) -> Result<(), VhostNetError>{
        for idx in 0..vq_pairs {
            let handle = &mut self.handles[idx];
            handle
                .set_owner()
                .map_err(|err| VhostNetError::VhostError(err))?;
            // self.device_info.acked_features()：这个方法调用返回设备已确认的特性。这些特性是设备和驱动程序在初始化期间协商的结果。
            // avail_features：这是当前可用的特性集，可能是来自驱动程序或设备的特性。
            // &（按位与操作符）：按位与操作符用于计算两个特性集合的交集。也就是说，features 变量将包含设备已确认并且当前可用的特性。
            let avail_features = handle.get_features().map_err(|err| VhostNetError::VhostError(err))?;
            let features = self.acked_features & avail_features;
            handle.set_features(features).map_err(|err| VhostNetError::VhostError(err))?;
            let tap = &self.taps[idx];
            tap.set_offload(virtio_features_to_tap_offload(self.acked_features))
                .map_err(|err| VhostNetError::VhostError(err))?;

        }
        Ok(())
    }
}

fn virtio_features_to_tap_offload(features: u64) -> u32 {
    let mut tap_offloads: u32 = 0;

    if features & (1 << VIRTIO_NET_F_GUEST_CSUM) != 0 {
        tap_offloads |= gen::TUN_F_CSUM;
    }
    if features & (1 << VIRTIO_NET_F_GUEST_TSO4) != 0 {
        tap_offloads |= gen::TUN_F_TSO4;
    }
    if features & (1 << VIRTIO_NET_F_GUEST_TSO6) != 0 {
        tap_offloads |= gen::TUN_F_TSO6;
    }
    if features & (1 << VIRTIO_NET_F_GUEST_ECN) != 0 {
        tap_offloads |= gen::TUN_F_TSO_ECN;
    }
    if features & (1 << VIRTIO_NET_F_GUEST_UFO) != 0 {
        tap_offloads |= gen::TUN_F_UFO;
    }

    tap_offloads
}

impl VirtioDevice for Net {
    fn avail_features(&self) -> u64 {
        self.avail_features
    }

    fn acked_features(&self) -> u64 {
        self.acked_features
    }

    fn set_acked_features(&mut self, acked_features: u64) {
        self.acked_features = acked_features;
    }

    fn device_type(&self) -> u32 {
        TYPE_NET
    }

    fn queues(&self) -> &[Queue] {
        &self.queues
    }

    fn queues_mut(&mut self) -> &mut [Queue] {
        &mut self.queues
    }

    fn queue_events(&self) -> &[EventFd] {
        &self.queue_evts
    }

    fn interrupt_evt(&self) -> &EventFd {
        &self.irq_trigger.irq_evt
    }

    fn interrupt_status(&self) -> Arc<AtomicU32> {
        self.irq_trigger.irq_status.clone()
    }

    fn read_config(&self, offset: u64, data: &mut [u8]) {
        // let config_space_bytes = self.config_space.as_slice();
        // let config_len = config_space_bytes.len() as u64;
        // if offset >= config_len {
        //     error!("Failed to read config space");
        //     return;
        // }
        // if let Some(end) = offset.checked_add(data.len() as u64) {
        //     // This write can't fail, offset and end are checked against config_len.
        //     data.write_all(
        //         &config_space_bytes[u64_to_usize(offset)..u64_to_usize(cmp::min(end, config_len))],
        //     )
        //         .unwrap();
        // }
    }

    fn write_config(&mut self, offset: u64, data: &[u8]) {
        // let config_space_bytes = self.config_space.as_mut_slice();
        // let start = usize::try_from(offset).ok();
        // let end = start.and_then(|s| s.checked_add(data.len()));
        // let Some(dst) = start
        //     .zip(end)
        //     .and_then(|(start, end)| config_space_bytes.get_mut(start..end))
        // else {
        //     error!("Failed to write config space");
        //     return;
        // };
        //
        // dst.copy_from_slice(data);
        // self.guest_mac = Some(self.config_space.guest_mac);
    }

    fn activate(&mut self, mem: GuestMemoryMmap) -> Result<(), ActivateError> {
        trace!(target: "vhost-net", "{}: Net::activate()", self.id);
        let vq_pairs = self.taps.len();

        self.do_device_activate(mem, vq_pairs);
        // self.setup_vhost_handle(&mem)
        //     .map_err(ActivateError::Vhost)?;
        //
        // if self.activate_evt.write(1).is_err() {
        //     error!("Net: Cannot write to activate_evt");
        //     return Err(ActivateError::BadActivate);
        // }
        // self.device_state = DeviceState::Activated(mem);
        Ok(())
    }

    fn is_activated(&self) -> bool {
        self.device_state.is_activated()
    }
}
