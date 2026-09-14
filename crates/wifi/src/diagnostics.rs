//! Bounded passive 802.11 observations. No decryption or packet-loss inference.

use core::fmt::{self, Write};

mod report;

/// Receive metadata supplied by the radio adapter.
#[derive(Clone, Copy, Debug)]
pub struct Reception {
    /// Monotonic host time in milliseconds.
    pub at_ms: u64,
    /// Primary receive channel.
    pub channel: u8,
    /// Signal measured at the scanner in dBm.
    pub rssi: i32,
    /// Driver-reported noise floor in dBm; uncalibrated.
    pub noise_floor: i32,
    /// Original frame length including FCS.
    pub wire_len: usize,
    /// Driver receive status, zero on success.
    pub rx_state: u32,
    /// Raw PHY rate code, meaningful for legacy b/g frames only.
    pub rate: u32,
    /// Raw C6 baseband format code.
    pub format: u32,
    /// Raw driver receive timestamp in microseconds (may wrap).
    pub timestamp_us: u64,
}

#[derive(Clone, Copy, Default)]
struct Signal {
    count: u32,
    sum: i64,
    min: i32,
    max: i32,
}

impl Signal {
    fn add(&mut self, value: i32) {
        if self.count == 0 {
            self.min = value;
            self.max = value;
        }
        self.count = self.count.saturating_add(1);
        self.sum = self.sum.saturating_add(i64::from(value));
        self.min = self.min.min(value);
        self.max = self.max.max(value);
    }

    fn average(&self) -> Option<i64> {
        (self.count > 0).then(|| self.sum / i64::from(self.count))
    }

    fn range(&self) -> Option<(i32, i64, i32)> {
        self.average().map(|average| (self.min, average, self.max))
    }
}

#[derive(Clone, Copy, Default)]
struct Traffic {
    frames: u32,
    bytes: u64,
    data: u32,
    retries: u32,
    management: u32,
    control: u32,
    protected: u32,
    power_save: u32,
    beacons: u32,
    signal: Signal,
    noise: Signal,
}

#[derive(Clone, Copy)]
struct Peer {
    mac: [u8; 6],
    bssid: Option<[u8; 6]>,
    channel: u8,
    last_ms: u64,
    last_tx_ms: Option<u64>,
    max_tx_gap_ms: u64,
    traffic: Traffic,
    received: u32,
    previous_rssi: Option<i64>,
    sequence: u16,
    rate: u32,
    format: u32,
}

#[derive(Clone, Copy)]
struct Ap {
    bssid: [u8; 6],
    ssid: [u8; 32],
    ssid_len: usize,
    channel: u8,
    advertised_channel: Option<u8>,
    last_ms: u64,
    rssi: i32,
    interval_tu: u16,
    capability: u16,
    rsn: bool,
    rsn_details: Option<Rsn>,
    wpa: bool,
    ht: bool,
    he: bool,
    secondary: Option<u8>,
    dtim: Option<u8>,
    load: Option<(u16, u8, u16)>,
    ies_incomplete: bool,
}

#[derive(Clone, Copy)]
struct Event {
    at_ms: u64,
    tx: [u8; 6],
    rx: [u8; 6],
    subtype: u8,
    code: Option<u16>,
    protected: bool,
}

/// Bounded analyzer. Identity tables persist; counters cover one report window.
///
/// Tables evict the oldest identity when full, except the optional target peer.
/// An absent target is reported as unobserved, never as disconnected.
pub struct Analyzer<const PEERS: usize, const APS: usize> {
    peers: [Option<Peer>; PEERS],
    aps: [Option<Ap>; APS],
    events: [Option<Event>; 16],
    target: Option<[u8; 6]>,
    channel: u8,
    started_ms: u64,
    traffic: Traffic,
    malformed: u32,
    truncated: u32,
    rx_errors: u32,
    other_channel: u32,
    evictions: u32,
    event_overflow: u32,
    last_timestamp_us: Option<u64>,
}

impl<const PEERS: usize, const APS: usize> Analyzer<PEERS, APS> {
    /// Creates empty tables and an optional target MAC to highlight and retain.
    pub const fn new(target: Option<[u8; 6]>) -> Self {
        Self {
            peers: [None; PEERS],
            aps: [None; APS],
            events: [None; 16],
            target,
            channel: 0,
            started_ms: 0,
            traffic: Traffic {
                frames: 0,
                bytes: 0,
                data: 0,
                retries: 0,
                management: 0,
                control: 0,
                protected: 0,
                power_save: 0,
                beacons: 0,
                signal: Signal {
                    count: 0,
                    sum: 0,
                    min: 0,
                    max: 0,
                },
                noise: Signal {
                    count: 0,
                    sum: 0,
                    min: 0,
                    max: 0,
                },
            },
            malformed: 0,
            truncated: 0,
            rx_errors: 0,
            other_channel: 0,
            evictions: 0,
            event_overflow: 0,
            last_timestamp_us: None,
        }
    }

    /// Starts a new observation window, retaining identities and previous RSSI.
    /// Capture must be stopped/drained before switching windows.
    pub fn begin_window(&mut self, channel: u8, at_ms: u64) {
        self.channel = channel;
        self.started_ms = at_ms;
        self.traffic = Traffic::default();
        self.events.fill(None);
        self.malformed = 0;
        self.truncated = 0;
        self.rx_errors = 0;
        self.other_channel = 0;
        self.evictions = 0;
        self.event_overflow = 0;
        self.last_timestamp_us = None;
        for peer in self.peers.iter_mut().flatten() {
            if peer.channel == channel {
                peer.previous_rssi = peer.traffic.signal.average().or(peer.previous_rssi);
                peer.traffic = Traffic::default();
                peer.received = 0;
            }
            // Reporting and channel changes both create blind intervals.
            peer.last_tx_ms = None;
            peer.max_tx_gap_ms = 0;
        }
    }

    /// Consumes a frame prefix including the MAC header. `wire_len` includes FCS.
    /// Invalid/error frames cannot update identities or management reason codes.
    pub fn observe(&mut self, capture: &[u8], rx: Reception) {
        if rx.channel != self.channel || rx.at_ms < self.started_ms {
            self.other_channel = self.other_channel.saturating_add(1);
            return;
        }
        self.traffic.frames = self.traffic.frames.saturating_add(1);
        self.traffic.bytes = self.traffic.bytes.saturating_add(rx.wire_len as u64);
        self.last_timestamp_us = Some(rx.timestamp_us);
        self.traffic.signal.add(rx.rssi);
        self.traffic.noise.add(rx.noise_floor);
        if capture.len() < rx.wire_len {
            self.truncated = self.truncated.saturating_add(1);
        }
        if rx.rx_state != 0 {
            self.rx_errors = self.rx_errors.saturating_add(1);
            return;
        }
        // FCS is not part of the management body or information-element list.
        let data = &capture[..capture.len().min(rx.wire_len.saturating_sub(4))];
        let Some(header) = Header::parse(data) else {
            self.malformed = self.malformed.saturating_add(1);
            return;
        };
        count(&mut self.traffic, &header);
        if let Some(tx) = header.tx
            && let Some(index) = self.peer_index(tx, rx)
        {
            let peer = self.peers[index].as_mut().unwrap();
            if peer.channel != rx.channel {
                peer.traffic = Traffic::default();
                peer.received = 0;
                peer.previous_rssi = None;
                peer.last_tx_ms = None;
            }
            peer.channel = rx.channel;
            peer.last_ms = rx.at_ms;
            if let Some(last) = peer.last_tx_ms {
                peer.max_tx_gap_ms = peer.max_tx_gap_ms.max(rx.at_ms.saturating_sub(last));
            }
            peer.last_tx_ms = Some(rx.at_ms);
            if header.bssid.is_some() {
                peer.bssid = header.bssid;
            }
            peer.sequence = header.sequence;
            peer.rate = rx.rate;
            peer.format = rx.format;
            peer.traffic.frames = peer.traffic.frames.saturating_add(1);
            peer.traffic.bytes = peer.traffic.bytes.saturating_add(rx.wire_len as u64);
            peer.traffic.signal.add(rx.rssi);
            count(&mut peer.traffic, &header);
        }
        if let Some(index) = self.peer_index(header.rx, rx) {
            let peer = self.peers[index].as_mut().unwrap();
            if peer.channel != rx.channel {
                peer.traffic = Traffic::default();
                peer.received = 0;
                peer.previous_rssi = None;
                peer.last_tx_ms = None;
            }
            peer.channel = rx.channel;
            peer.received = peer.received.saturating_add(1);
            peer.last_ms = rx.at_ms;
        }
        if header.kind == 0 {
            if matches!(header.subtype, 5 | 8) && data.len() >= 36 && !header.protected {
                self.observe_ap(data, &header, rx);
            }
            if matches!(header.subtype, 0..=3 | 10..=12) {
                let code = if header.protected {
                    None
                } else {
                    match header.subtype {
                        1 | 3 => le16(data, 26),
                        11 => le16(data, 28),
                        10 | 12 => le16(data, 24),
                        _ => None,
                    }
                };
                let event = Event {
                    at_ms: rx.at_ms,
                    tx: header.tx.unwrap(),
                    rx: header.rx,
                    subtype: header.subtype,
                    code,
                    protected: header.protected,
                };
                if let Some(slot) = self.events.iter_mut().find(|slot| slot.is_none()) {
                    *slot = Some(event);
                } else {
                    self.events.rotate_left(1);
                    self.events[15] = Some(event);
                    self.event_overflow = self.event_overflow.saturating_add(1);
                }
            }
        }
    }

    fn peer_index(&mut self, mac: [u8; 6], rx: Reception) -> Option<usize> {
        if mac[0] & 1 != 0 || mac == [0; 6] {
            return None;
        }
        if let Some(i) = self
            .peers
            .iter()
            .position(|p| p.is_some_and(|p| p.mac == mac))
        {
            return Some(i);
        }
        let index = self.peers.iter().position(Option::is_none).or_else(|| {
            self.peers
                .iter()
                .enumerate()
                .filter(|(_, p)| p.is_some_and(|p| Some(p.mac) != self.target))
                .min_by_key(|(_, p)| p.unwrap().last_ms)
                .map(|(i, _)| i)
        })?;
        if self.peers[index].is_some() {
            self.evictions = self.evictions.saturating_add(1);
        }
        self.peers[index] = Some(Peer {
            mac,
            bssid: None,
            channel: rx.channel,
            last_ms: rx.at_ms,
            last_tx_ms: None,
            max_tx_gap_ms: 0,
            traffic: Traffic::default(),
            received: 0,
            previous_rssi: None,
            sequence: 0,
            rate: 0,
            format: 0,
        });
        Some(index)
    }

    fn observe_ap(&mut self, data: &[u8], header: &Header, rx: Reception) {
        let Some(bssid) = header.bssid else {
            return;
        };
        let existing = self
            .aps
            .iter()
            .position(|ap| ap.is_some_and(|ap| ap.bssid == bssid));
        let index = existing
            .or_else(|| self.aps.iter().position(Option::is_none))
            .or_else(|| {
                self.aps
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, ap)| ap.unwrap().last_ms)
                    .map(|(i, _)| i)
            });
        let Some(index) = index else {
            return;
        };
        if existing.is_none() && self.aps[index].is_some() {
            self.evictions = self.evictions.saturating_add(1);
        }
        let mut ap = Ap {
            bssid,
            ssid: [0; 32],
            ssid_len: 0,
            channel: rx.channel,
            last_ms: rx.at_ms,
            rssi: rx.rssi,
            advertised_channel: None,
            interval_tu: le16(data, 32).unwrap(),
            capability: le16(data, 34).unwrap(),
            rsn: false,
            rsn_details: None,
            wpa: false,
            ht: false,
            he: false,
            secondary: None,
            dtim: None,
            load: None,
            ies_incomplete: data.len() < rx.wire_len.saturating_sub(4),
        };
        let mut ies = &data[36..];
        while !ies.is_empty() {
            if ies.len() < 2 || ies.len() < 2 + usize::from(ies[1]) {
                ap.ies_incomplete = true;
                break;
            }
            let id = ies[0];
            let value = &ies[2..2 + usize::from(ies[1])];
            match id {
                3 if value.len() == 1 && (1..=14).contains(&value[0]) => {
                    ap.advertised_channel = Some(value[0]);
                }
                0 if value.len() <= 32 => {
                    ap.ssid[..value.len()].copy_from_slice(value);
                    ap.ssid_len = value.len();
                }
                5 if value.len() >= 4 => ap.dtim = Some(value[1]),
                11 if value.len() == 5 => {
                    ap.load = Some((le16(value, 0).unwrap(), value[2], le16(value, 3).unwrap()))
                }
                45 if value.len() == 26 => ap.ht = true,
                48 => {
                    ap.rsn = true;
                    ap.rsn_details = Rsn::parse(value);
                }
                61 if value.len() >= 2 => {
                    ap.secondary = Some(value[1] & 3);
                    if ap.advertised_channel.is_none() && (1..=14).contains(&value[0]) {
                        ap.advertised_channel = Some(value[0]);
                    }
                }
                221 if value.starts_with(&[0, 0x50, 0xf2, 1]) => ap.wpa = true,
                255 if value.first() == Some(&35) => ap.he = true,
                _ => {}
            }
            ies = &ies[2 + value.len()..];
        }
        self.aps[index] = Some(ap);
    }

    /// Prints raw detail including all retained APs and peers, even stale ones.
    /// `dropped` is queue loss for this window; unknown radio loss is separate.
    /// `gap_before_ms` includes reporting/tuning time preceding this capture.
    pub fn write_verbose_report(
        &self,
        out: &mut impl Write,
        now_ms: u64,
        dropped: u32,
        gap_before_ms: u64,
    ) -> fmt::Result {
        let elapsed = now_ms.saturating_sub(self.started_ms).max(1);
        writeln!(out, "\n========== WIFI DIAGNOSTICS BEGIN ==========")?;
        writeln!(
            out,
            "uptime_ms={now_ms} channel={} observed_ms={elapsed} capture_gap_before_window_ms={gap_before_ms} peer_capacity={PEERS} ap_capacity={APS}",
            self.channel
        )?;
        writeln!(
            out,
            "capture: frames={} bytes={} frames/s={} bytes/s={} management={} data={} control={}",
            self.traffic.frames,
            self.traffic.bytes,
            u64::from(self.traffic.frames) * 1000 / elapsed,
            self.traffic.bytes.saturating_mul(1000) / elapsed,
            self.traffic.management,
            self.traffic.data,
            self.traffic.control
        )?;
        write!(
            out,
            "radio: rssi_min/avg/max={:?}dBm noise_avg_raw={:?}dBm last_rx_timestamp_us={:?} ",
            self.traffic.signal.range(),
            self.traffic.noise.average(),
            self.last_timestamp_us
        )?;
        write_retry(out, self.traffic.retries, self.traffic.data)?;
        writeln!(
            out,
            "capture_quality: queue_dropped={dropped} prefix_truncated={} malformed={} delivered_rx_errors={} off_window={} evictions={} omitted_events={}",
            self.truncated,
            self.malformed,
            self.rx_errors,
            self.other_channel,
            self.evictions,
            self.event_overflow
        )?;
        writeln!(
            out,
            "APs (retained; age shows stale/off-channel records; flags are advertised):"
        )?;
        for ap in self.aps.iter().flatten() {
            write!(
                out,
                " AP {} rx_ch={} age_ms={} rssi={} ssid=",
                Mac(ap.bssid),
                ap.channel,
                now_ms.saturating_sub(ap.last_ms),
                ap.rssi
            )?;
            write_ssid(out, &ap.ssid[..ap.ssid_len])?;
            writeln!(
                out,
                " advertised_ch={:?} beacon_tu={} capability=0x{:04x} privacy={} rsn={} wpa={} ht={} he={} secondary_raw={:?} dtim={:?} bss_load(stations,util/255,admission32us)={:?} ies_incomplete={}",
                ap.advertised_channel,
                ap.interval_tu,
                ap.capability,
                ap.capability & 16 != 0,
                ap.rsn,
                ap.wpa,
                ap.ht,
                ap.he,
                ap.secondary,
                ap.dtim,
                ap.load,
                ap.ies_incomplete
            )?;
            if let Some(rsn) = ap.rsn_details {
                writeln!(
                    out,
                    "  rsn: group_cipher_raw={:08x} pairwise_type_bits=0x{:08x} akm_type_bits=0x{:08x} unknown_suites={} pmf_capable={} pmf_required={}",
                    rsn.group,
                    rsn.pairwise,
                    rsn.akm,
                    rsn.unknown,
                    rsn.capabilities & 0x80 != 0,
                    rsn.capabilities & 0x40 != 0
                )?;
            }
        }
        writeln!(
            out,
            "Peers (this channel; TX signal is measured at scanner; RX means addressed frames, not confirmed delivery):"
        )?;
        for peer in self
            .peers
            .iter()
            .flatten()
            .filter(|peer| peer.channel == self.channel)
        {
            let t = peer.traffic;
            write!(
                out,
                " {}{} bssid={:?} age_ms={} tx={} addressed={} bytes={} rssi_min/avg/max={:?} delta_db={:?} max_observed_tx_gap_ms={} last_seq/format/rate_raw={:?} protected={} power_save={} beacons={} ",
                if Some(peer.mac) == self.target {
                    "TARGET "
                } else {
                    ""
                },
                Mac(peer.mac),
                peer.bssid.map(Mac),
                now_ms.saturating_sub(peer.last_ms),
                t.frames,
                peer.received,
                t.bytes,
                t.signal.range(),
                t.signal
                    .average()
                    .zip(peer.previous_rssi)
                    .map(|(a, b)| a - b),
                peer.max_tx_gap_ms,
                (t.frames > 0).then_some((peer.sequence, peer.format, peer.rate)),
                t.protected,
                t.power_save,
                t.beacons
            )?;
            write_retry(out, t.retries, t.data)?;
            if t.signal.count >= 10 && t.signal.average().is_some_and(|avg| avg < -75) {
                writeln!(
                    out,
                    "  warning=weak_signal_at_scanner samples={}",
                    t.signal.count
                )?;
            }
            if t.data >= 20 && u64::from(t.retries) * 100 >= u64::from(t.data) * 20 {
                writeln!(
                    out,
                    "  warning=high_observed_retry_fraction samples={}",
                    t.data
                )?;
            }
        }
        if let Some(target) = self.target {
            let seen = self.peers.iter().flatten().find(|p| p.mac == target);
            writeln!(
                out,
                "target={} last_observed_age_ms={:?} last_channel={:?}; silence is not proof of outage",
                Mac(target),
                seen.map(|p| now_ms.saturating_sub(p.last_ms)),
                seen.map(|p| p.channel)
            )?;
        }
        writeln!(
            out,
            "Management events (observed, unauthenticated; code is status for responses/auth, reason for disconnects):"
        )?;
        for event in self.events.iter().flatten() {
            writeln!(
                out,
                " event t={} {} -> {} kind={} code={:?} protected={}{}",
                event.at_ms,
                Mac(event.tx),
                Mac(event.rx),
                event_name(event.subtype),
                event.code,
                event.protected,
                if Some(event.tx) == self.target || Some(event.rx) == self.target {
                    " TARGET"
                } else {
                    ""
                }
            )?;
        }
        writeln!(
            out,
            "limits: 2.4GHz/one channel; capture-filter limited; CRC/control completeness, true loss, collisions, airtime utilization, device-side RSSI, DHCP/DNS/MQTT health=unknown"
        )?;
        writeln!(out, "========== WIFI DIAGNOSTICS END ============")
    }
}

fn write_retry(out: &mut impl Write, retries: u32, data: u32) -> fmt::Result {
    if data == 0 {
        writeln!(out, "data_retries=0/0 (n/a)")
    } else {
        writeln!(
            out,
            "data_retries={retries}/{data} ({}.{:01}%)",
            u64::from(retries) * 100 / data as u64,
            u64::from(retries) * 1000 / data as u64 % 10
        )
    }
}

fn write_ssid(out: &mut impl Write, bytes: &[u8]) -> fmt::Result {
    out.write_char('"')?;
    for byte in bytes {
        match byte {
            b' '..=b'~' if !matches!(byte, b'"' | b'\\') => out.write_char(char::from(*byte))?,
            _ => write!(out, "\\x{byte:02x}")?,
        }
    }
    out.write_char('"')
}

/// MAC address formatted without allocation.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Mac(pub [u8; 6]);

impl fmt::Display for Mac {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let a = self.0;
        write!(
            out,
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            a[0], a[1], a[2], a[3], a[4], a[5]
        )
    }
}
impl fmt::Debug for Mac {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, out)
    }
}

fn event_name(subtype: u8) -> &'static str {
    match subtype {
        0 => "association_request",
        1 => "association_response",
        2 => "reassociation_request",
        3 => "reassociation_response",
        10 => "disassociation",
        11 => "authentication",
        12 => "deauthentication",
        _ => "unknown",
    }
}

struct Header {
    kind: u8,
    subtype: u8,
    rx: [u8; 6],
    tx: Option<[u8; 6]>,
    bssid: Option<[u8; 6]>,
    sequence: u16,
    retry: bool,
    protected: bool,
    power_save: bool,
}

impl Header {
    fn parse(data: &[u8]) -> Option<Self> {
        let fc = le16(data, 0)?;
        if fc & 3 != 0 {
            return None;
        }
        let kind = ((fc >> 2) & 3) as u8;
        let subtype = ((fc >> 4) & 15) as u8;
        let ds = (fc >> 8) & 3;
        let required = match kind {
            0 => 24,
            1 => {
                if matches!(subtype, 12 | 13) {
                    10
                } else {
                    16
                }
            }
            2 => 24 + if ds == 3 { 6 } else { 0 } + if subtype & 8 != 0 { 2 } else { 0 },
            _ => return None,
        };
        if data.len() < required {
            return None;
        }
        let rx = data[4..10].try_into().ok()?;
        let tx = (required >= 16).then(|| data[10..16].try_into().unwrap());
        let bssid = if kind == 0 {
            Some(data[16..22].try_into().ok()?)
        } else if kind == 2 {
            match ds {
                0 => Some(data[16..22].try_into().ok()?),
                1 => Some(rx),
                2 => tx,
                _ => None,
            }
        } else {
            None
        };
        // Wildcard probe BSSIDs do not identify an association.
        let bssid = bssid.filter(|mac: &[u8; 6]| mac[0] & 1 == 0 && *mac != [0; 6]);
        Some(Self {
            kind,
            subtype,
            rx,
            tx,
            bssid,
            sequence: if kind == 1 { 0 } else { le16(data, 22)? >> 4 },
            retry: fc & 0x0800 != 0,
            protected: fc & 0x4000 != 0,
            power_save: fc & 0x1000 != 0,
        })
    }
}

fn le16(bytes: &[u8], index: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(index..index + 2)?.try_into().ok()?,
    ))
}

#[derive(Clone, Copy)]
struct Rsn {
    group: u32,
    pairwise: u32,
    akm: u32,
    unknown: u16,
    capabilities: u16,
}

impl Rsn {
    fn parse(bytes: &[u8]) -> Option<Self> {
        if le16(bytes, 0)? != 1 {
            return None;
        }
        let group = u32::from_be_bytes(bytes.get(2..6)?.try_into().ok()?);
        let mut index = 6;
        let mut unknown = 0;
        let pairwise = Self::suites(bytes, &mut index, &mut unknown)?;
        let akm = Self::suites(bytes, &mut index, &mut unknown)?;
        // Capabilities are optional, but a single trailing byte is malformed.
        let capabilities = if bytes.len() == index {
            0
        } else {
            le16(bytes, index)?
        };
        Some(Self {
            group,
            pairwise,
            akm,
            unknown,
            capabilities,
        })
    }

    fn suites(bytes: &[u8], index: &mut usize, unknown: &mut u16) -> Option<u32> {
        let count = usize::from(le16(bytes, *index)?);
        *index += 2;
        let list = bytes.get(*index..*index + count * 4)?;
        *index += count * 4;
        let mut mask = 0;
        for suite in list.chunks_exact(4) {
            if suite[..3] == [0, 0x0f, 0xac] && suite[3] < 32 {
                mask |= 1 << suite[3];
            } else {
                *unknown = unknown.saturating_add(1);
            }
        }
        Some(mask)
    }
}

fn count(t: &mut Traffic, h: &Header) {
    match h.kind {
        0 => t.management = t.management.saturating_add(1),
        1 => t.control = t.control.saturating_add(1),
        2 => {
            t.data = t.data.saturating_add(1);
            t.retries = t.retries.saturating_add(u32::from(h.retry));
        }
        _ => {}
    }
    t.protected = t.protected.saturating_add(u32::from(h.protected));
    t.power_save = t.power_save.saturating_add(u32::from(h.power_save));
    t.beacons = t
        .beacons
        .saturating_add(u32::from(h.kind == 0 && h.subtype == 8));
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::{string::String, vec, vec::Vec};

    const DEVICE: [u8; 6] = [2, 1, 2, 3, 4, 5];
    const AP: [u8; 6] = [4, 1, 2, 3, 4, 5];

    fn frame(fc: u16, body: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; 24];
        bytes[..2].copy_from_slice(&fc.to_le_bytes());
        bytes[4..10].copy_from_slice(&AP);
        bytes[10..16].copy_from_slice(&DEVICE);
        bytes[16..22].copy_from_slice(&AP);
        bytes[22..24].copy_from_slice(&0x1230_u16.to_le_bytes());
        bytes.extend_from_slice(body);
        bytes.extend_from_slice(&[0; 4]);
        bytes
    }

    fn rx(bytes: &[u8], at_ms: u64) -> Reception {
        Reception {
            at_ms,
            channel: 6,
            rssi: -80,
            noise_floor: -95,
            wire_len: bytes.len(),
            rx_state: 0,
            rate: 11,
            format: 0,
            timestamp_us: at_ms * 1000,
        }
    }

    fn analyzer() -> Analyzer<8, 4> {
        let mut analyzer = Analyzer::new(Some(DEVICE));
        analyzer.begin_window(6, 0);
        analyzer
    }

    #[test]
    fn retry_denominator_only_includes_observed_data_and_signal_belongs_to_transmitter() {
        let mut a = analyzer();
        for i in 0..20 {
            let bytes = frame(if i < 5 { 0x0908 } else { 0x0108 }, &[]);
            a.observe(&bytes, rx(&bytes, i));
        }
        let management = frame(0x08c0, &[4, 0]);
        a.observe(&management, rx(&management, 25));
        assert_eq!((a.traffic.data, a.traffic.retries), (20, 5));
        let ap = a.peers.iter().flatten().find(|p| p.mac == AP).unwrap();
        assert_eq!(ap.traffic.signal.average(), None);
        assert_eq!(ap.received, 21);
        let device = a.peers.iter().flatten().find(|p| p.mac == DEVICE).unwrap();
        assert_eq!(device.bssid, Some(AP));
        assert_eq!(device.sequence, 0x123);
        let mut report = String::new();
        a.write_verbose_report(&mut report, 5000, 3, 42).unwrap();
        assert!(report.contains("data_retries=5/20 (25.0%)"));
        assert!(report.contains("warning=high_observed_retry_fraction"));
        assert!(report.contains("queue_dropped=3"));
        assert!(report.contains("capture_gap_before_window_ms=42"));
    }

    #[test]
    fn direction_mapping_and_short_control_frames() {
        for (fc, expected) in [
            (0x0008, Some(AP)),
            (0x0108, Some(AP)),
            (0x0208, Some(DEVICE)),
            (0x0308, None),
        ] {
            let bytes = frame(fc, &[0; 6]);
            assert_eq!(
                Header::parse(&bytes[..bytes.len() - 4]).unwrap().bssid,
                expected
            );
        }
        let mut ack = vec![0; 10];
        ack[0] = 0xd4;
        ack[4..10].copy_from_slice(&DEVICE);
        let header = Header::parse(&ack).unwrap();
        assert_eq!(header.tx, None);
        assert_eq!(header.rx, DEVICE);
        assert!(Header::parse(&ack[..9]).is_none());
        let qos = frame(0x0388, &[]);
        assert!(Header::parse(&qos[..24]).is_none());
    }

    #[test]
    fn protected_or_missing_management_bodies_never_become_reason_codes() {
        let mut a = analyzer();
        for (fc, body) in [
            (0x00c0, &[15, 0][..]),
            (0x40c0, &[15, 0][..]),
            (0x00c0, &[][..]),
        ] {
            let bytes = frame(fc, body);
            a.observe(&bytes, rx(&bytes, 1));
        }
        assert_eq!(a.events[0].unwrap().code, Some(15));
        assert_eq!(a.events[1].unwrap().code, None);
        assert!(a.events[1].unwrap().protected);
        assert_eq!(a.events[2].unwrap().code, None); // FCS zeros aren't a reason.
    }

    #[test]
    fn beacon_metadata_and_binary_ssids_are_bounded_and_log_safe() {
        let mut body = vec![0; 12];
        body[8..10].copy_from_slice(&100_u16.to_le_bytes());
        body[10] = 0x11;
        body.extend_from_slice(&[
            0, 4, b'a', b'\n', 0x1b, 0xff, 5, 4, 0, 3, 0, 0, 11, 5, 7, 0, 128, 10, 0, 61, 2, 6, 1,
        ]);
        let bytes = frame(0x0080, &body);
        let mut a = analyzer();
        a.observe(&bytes, rx(&bytes, 1));
        let ap = a.aps[0].unwrap();
        assert_eq!(ap.dtim, Some(3));
        assert_eq!(ap.load, Some((7, 128, 10)));
        assert_eq!(ap.interval_tu, 100);
        assert!(!ap.ies_incomplete);
        let mut report = String::new();
        a.write_verbose_report(&mut report, 5000, 0, 0).unwrap();
        assert!(report.contains(r#"ssid="a\x0a\x1b\xff""#));
        assert!(!report.contains('\x1b'));
        a.observe(&bytes[..bytes.len() - 6], rx(&bytes, 2));
        assert!(a.aps[0].unwrap().ies_incomplete);
        assert_eq!(a.truncated, 1);
    }

    #[test]
    fn rsn_parses_transition_mode_and_pmf_and_rejects_truncated_lists() {
        let bytes = [
            1, 0, 0, 15, 172, 4, 1, 0, 0, 15, 172, 4, 2, 0, 0, 15, 172, 2, 0, 15, 172, 8, 0x80, 0,
        ];
        let rsn = Rsn::parse(&bytes).unwrap();
        assert_eq!(rsn.pairwise, 1 << 4);
        assert_eq!(rsn.akm, (1 << 2) | (1 << 8));
        assert_eq!(rsn.capabilities, 0x80);
        for end in 0..22 {
            assert!(Rsn::parse(&bytes[..end]).is_none());
        }
        assert!(Rsn::parse(&bytes[..22]).is_some()); // optional capabilities
        assert!(Rsn::parse(&bytes[..23]).is_none());
    }

    #[test]
    fn channel_windows_reset_counters_and_do_not_infer_off_channel_gaps() {
        let mut a = analyzer();
        let bytes = frame(0x0108, &[]);
        a.observe(&bytes, rx(&bytes, 10));
        a.observe(&bytes, rx(&bytes, 30));
        a.begin_window(1, 5000);
        a.observe(&bytes, rx(&bytes, 5010));
        assert_eq!(a.other_channel, 1);
        assert_eq!(a.traffic.frames, 0);
        a.begin_window(6, 10000);
        let mut reception = rx(&bytes, 10001);
        reception.rssi = -70;
        a.observe(&bytes, reception);
        let peer = a.peers.iter().flatten().find(|p| p.mac == DEVICE).unwrap();
        assert_eq!(peer.max_tx_gap_ms, 0);
        assert_eq!(peer.traffic.frames, 1);
        assert_eq!(peer.previous_rssi, Some(-80));
        a.begin_window(6, 15000);
        a.observe(&bytes, rx(&bytes, 15001));
        assert_eq!(a.peers[0].unwrap().max_tx_gap_ms, 0);
    }

    #[test]
    fn error_frames_cannot_pollute_identities_and_no_samples_is_not_healthy() {
        let mut a = analyzer();
        let bytes = frame(0x00c0, &[15, 0]);
        let mut reception = rx(&bytes, 1);
        reception.rx_state = 1;
        a.observe(&bytes, reception);
        assert_eq!(a.rx_errors, 1);
        assert!(a.peers.iter().all(Option::is_none));
        assert!(a.events.iter().all(Option::is_none));
        let mut report = String::new();
        a.write_verbose_report(&mut report, 5000, 0, 0).unwrap();
        assert!(report.contains("data_retries=0/0 (n/a)"));
        assert!(report.contains("last_observed_age_ms=None"));
        assert!(!report.contains("warning="));
    }

    #[test]
    fn bounded_tables_pin_target_and_account_for_event_overflow() {
        let mut a = Analyzer::<2, 1>::new(Some(DEVICE));
        a.begin_window(6, 0);
        for i in 0..20 {
            let mut bytes = frame(0x00c0, &[4, 0]);
            if i != 0 {
                bytes[10] = 6;
                bytes[11] = i;
            }
            a.observe(&bytes, rx(&bytes, u64::from(i)));
        }
        assert!(a.peers.iter().flatten().any(|p| p.mac == DEVICE));
        assert!(a.evictions > 0);
        assert_eq!(a.event_overflow, 4);
        assert_eq!(a.events[0].unwrap().at_ms, 4);
    }

    #[test]
    fn arbitrary_frame_prefixes_and_zero_capacity_tables_do_not_panic() {
        let mut a = Analyzer::<0, 0>::new(None);
        a.begin_window(6, 0);
        let mut bytes = [0; 128];
        let mut seed = 42_u32;
        for _ in 0..500 {
            for byte in &mut bytes {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                *byte = (seed >> 24) as u8;
            }
            for end in 0..bytes.len() {
                a.observe(&bytes[..end], rx(&bytes, 1));
            }
        }
    }

    #[test]
    fn readable_report_omits_stale_rows_and_shows_security_and_advertised_channel() {
        let mut a = analyzer();
        // Cache an old AP on the same receive channel.
        let old = frame(0x0080, &[0; 12]);
        a.observe(&old, rx(&old, 1));
        a.begin_window(6, 5000);
        let mut body = vec![0; 12];
        body.extend_from_slice(&[
            0, 7, b'I', b'o', b'T', b' ', b'L', b'a', b'b', 3, 1, 5, 11, 5, 7, 0, 128, 10, 0, 48,
            24, 1, 0, 0, 15, 172, 4, 1, 0, 0, 15, 172, 4, 2, 0, 0, 15, 172, 2, 0, 15, 172, 8, 0x80,
            0,
        ]);
        let mut beacon = frame(0x0080, &body);
        let new_ap = [6, 1, 2, 3, 4, 5];
        beacon[4..10].fill(0xff);
        beacon[10..16].copy_from_slice(&new_ap);
        beacon[16..22].copy_from_slice(&new_ap);
        let mut reception = rx(&beacon, 5100);
        reception.rssi = -61;
        a.observe(&beacon, reception);
        let mut report = String::new();
        a.write_report(&mut report, 10000, 2, 49).unwrap();
        assert!(report.contains("NETWORKS HEARD THIS WINDOW: 1 (1 cached networks omitted)"));
        assert!(report.contains("Listening CH 6"));
        assert!(report.contains("06:01:02:03:04:05  5  -61 WPA2/WPA3"));
        assert!(report.contains(" 50.1%"));
        assert!(report.contains("Capture gap 49ms"));
        assert!(report.contains("2 queue drops"));
        assert!(report.contains("not seen this window"));
        assert!(report.contains("no data frames captured"));
        assert!(!report.contains("Some("));
        assert!(!report.contains("None"));
        assert!(!report.contains("pairwise_type_bits"));
        assert!(!report.contains("04:01:02:03:04:05"));
        std::println!("{report}");
    }

    #[test]
    fn readable_report_orders_active_signals_and_retains_warnings() {
        let mut a = Analyzer::<8, 4>::new(None);
        a.begin_window(6, 0);
        for i in 0..20 {
            let bytes = frame(0x0908, &[]);
            a.observe(&bytes, rx(&bytes, i));
        }
        let mut bytes = frame(0x0108, &[]);
        bytes[10] = 6;
        let mut reception = rx(&bytes, 30);
        reception.rssi = -50;
        a.observe(&bytes, reception);
        let mut report = String::new();
        a.write_report(&mut report, 5000, 0, 0).unwrap();
        assert!(report.contains("Weak signal at scanner"));
        assert!(report.contains("High data retries"));
        assert!(report.contains("100.0%"));
        let table = report.split("ACTIVE RADIOS:").nth(1).unwrap();
        assert!(
            table.find("06:01:02:03:04:05").unwrap() < table.find("02:01:02:03:04:05").unwrap()
        );
    }

    #[test]
    fn wildcard_probe_bssid_is_not_an_association() {
        let mut probe = frame(0x0040, &[]);
        probe[16..22].fill(0xff);
        assert_eq!(Header::parse(&probe).unwrap().bssid, None);
    }
}
