//! Human-readable serial reports; raw detail remains opt-in.

use super::*;

impl<const PEERS: usize, const APS: usize> Analyzer<PEERS, APS> {
    /// Writes a readable snapshot of this window with strongest signals first.
    /// Cached identities without activity are counted but omitted from tables.
    /// `dropped` counts queue loss; `gap_before_ms` is the preceding blind gap.
    pub fn write_report(
        &self,
        out: &mut impl Write,
        now_ms: u64,
        dropped: u32,
        gap_before_ms: u64,
    ) -> fmt::Result {
        let elapsed = now_ms.saturating_sub(self.started_ms).max(1);
        let mut aps = [None; APS];
        let mut ap_count = 0;
        for ap in self
            .aps
            .iter()
            .flatten()
            .filter(|ap| ap.channel == self.channel && ap.last_ms >= self.started_ms)
        {
            aps[ap_count] = Some(ap);
            ap_count += 1;
        }
        aps[..ap_count].sort_unstable_by_key(|ap| core::cmp::Reverse(ap.unwrap().rssi));
        let cached = self.aps.iter().flatten().count() - ap_count;
        let mut peers = [None; PEERS];
        let mut peer_count = 0;
        for peer in self
            .peers
            .iter()
            .flatten()
            .filter(|p| p.channel == self.channel && (p.traffic.frames > 0 || p.received > 0))
        {
            peers[peer_count] = Some(peer);
            peer_count += 1;
        }
        peers[..peer_count].sort_unstable_by_key(|p| {
            let p = p.unwrap();
            (
                Some(p.mac) != self.target,
                core::cmp::Reverse(p.traffic.signal.average()),
            )
        });

        writeln!(
            out,
            "\n===================== WI-FI DIAGNOSTICS ====================="
        )?;
        writeln!(
            out,
            "Uptime {}s | Listening CH {} | Window {}ms | Capture gap {}ms",
            now_ms / 1000,
            self.channel,
            elapsed,
            gap_before_ms
        )?;
        writeln!(
            out,
            "Traffic: {} frames | {} data | {} management | {} bytes/s",
            self.traffic.frames,
            self.traffic.data,
            self.traffic.management,
            self.traffic.bytes.saturating_mul(1000) / elapsed
        )?;
        writeln!(
            out,
            "Data retries: {} ({} / {}) | Signal avg: {} dBm | Noise: {} dBm (uncalibrated)",
            Percent(self.traffic.retries, self.traffic.data),
            self.traffic.retries,
            self.traffic.data,
            Value(self.traffic.signal.average()),
            Value(self.traffic.noise.average())
        )?;
        writeln!(
            out,
            "Capture: {} queue drops | {} short prefixes | {} malformed | {} RX errors delivered",
            dropped, self.truncated, self.malformed, self.rx_errors
        )?;
        if self.evictions > 0 || self.event_overflow > 0 || self.other_channel > 0 {
            writeln!(
                out,
                "Coverage: {} table evictions | {} omitted events | {} off-window frames",
                self.evictions, self.event_overflow, self.other_channel
            )?;
        }

        let mut warnings = 0;
        for peer in peers[..peer_count].iter().flatten() {
            let t = peer.traffic;
            if t.signal.count >= 10 && t.signal.average().is_some_and(|avg| avg < -75) {
                writeln!(
                    out,
                    "! Weak signal at scanner: {} ({} dBm, {} samples)",
                    Mac(peer.mac),
                    Value(t.signal.average()),
                    t.signal.count
                )?;
                warnings += 1;
            }
            if t.data >= 20 && u64::from(t.retries) * 100 >= u64::from(t.data) * 20 {
                writeln!(
                    out,
                    "! High data retries: {} ({}; {} / {})",
                    Mac(peer.mac),
                    Percent(t.retries, t.data),
                    t.retries,
                    t.data
                )?;
                warnings += 1;
            }
        }
        if self.traffic.data == 0 {
            writeln!(
                out,
                "Assessment: no data frames captured; device connection health is unknown."
            )?;
        } else if warnings == 0 {
            writeln!(
                out,
                "Assessment: no signal/retry threshold warnings; connection health is unverified."
            )?;
        }
        if let Some(target) = self.target {
            match self.peers.iter().flatten().find(|p| p.mac == target) {
                Some(p) => writeln!(
                    out,
                    "Target {}: last observed {}s ago on receive CH {}{}",
                    Mac(target),
                    now_ms.saturating_sub(p.last_ms) / 1000,
                    p.channel,
                    if p.last_ms < self.started_ms {
                        " (not seen this window)"
                    } else {
                        ""
                    }
                )?,
                None => writeln!(out, "Target {}: not observed yet", Mac(target))?,
            }
        }

        writeln!(
            out,
            "\nNETWORKS HEARD THIS WINDOW: {ap_count} ({cached} cached networks omitted)"
        )?;
        if ap_count > 0 {
            writeln!(
                out,
                " {:<17} {:>2} {:>4} {:<14} {:<3} {:>6} {:>7} NAME",
                "BSSID", "CH", "dBm", "SECURITY", "PMF", "LOAD", "CLIENTS"
            )?;
            for ap in aps[..ap_count].iter().flatten() {
                write!(
                    out,
                    " {} {:>2} {:>4} {:<14} {:<3} {} {:>7} ",
                    Mac(ap.bssid),
                    Value(ap.advertised_channel),
                    ap.rssi,
                    security(ap),
                    pmf(ap),
                    Percent(
                        ap.load.map_or(0, |l| u32::from(l.1)),
                        if ap.load.is_some() { 255 } else { 0 }
                    ),
                    Value(ap.load.map(|l| l.0))
                )?;
                if ap.ssid_len == 0 {
                    out.write_str("<hidden>")?;
                } else {
                    write_ssid(out, &ap.ssid[..ap.ssid_len])?;
                }
                if ap.ies_incomplete {
                    out.write_str(" *")?;
                }
                writeln!(out)?;
            }
            writeln!(
                out,
                "CH = advertised channel; LOAD = AP-reported use; PMF = req/opt/no; * = partial metadata."
            )?;
        }

        writeln!(
            out,
            "\nACTIVE RADIOS: {peer_count} (APs and clients; strongest TX signal first)"
        )?;
        if peer_count > 0 {
            writeln!(
                out,
                " {:<17} {:>4} {:>4} {:>8} {:>6} {:>6} NOTES",
                "MAC", "dBm", "TX", "TO-RADIO", "RETRY%", "AGEms"
            )?;
            for peer in peers[..peer_count].iter().flatten() {
                let t = peer.traffic;
                writeln!(
                    out,
                    " {} {:>4} {:>4} {:>8} {} {:>6} {}{}",
                    Mac(peer.mac),
                    Value(t.signal.average()),
                    t.frames,
                    peer.received,
                    Percent(t.retries, t.data),
                    now_ms.saturating_sub(peer.last_ms),
                    if Some(peer.mac) == self.target {
                        "TARGET "
                    } else {
                        ""
                    },
                    if self.aps.iter().flatten().any(|ap| ap.bssid == peer.mac) {
                        "AP"
                    } else {
                        ""
                    }
                )?;
            }
            writeln!(
                out,
                "TX = captured transmissions; TO-RADIO = addressed frames, not confirmed delivery."
            )?;
        }

        let events = self.events.iter().flatten().count();
        writeln!(out, "\nCONNECTION EVENTS: {events}")?;
        for event in self.events.iter().flatten() {
            write!(
                out,
                " {}ms ago | {} -> {} | {}",
                now_ms.saturating_sub(event.at_ms),
                Mac(event.tx),
                Mac(event.rx),
                event_name(event.subtype)
            )?;
            if event.protected {
                out.write_str(" | protected body; code unavailable")?;
            } else if let Some(code) = event.code {
                let kind = if matches!(event.subtype, 10 | 12) {
                    "reason"
                } else {
                    "status"
                };
                write!(out, " | {kind} {code}")?;
                if kind == "status" {
                    out.write_str(if code == 0 {
                        " (success)"
                    } else {
                        " (rejected)"
                    })?;
                }
            }
            if Some(event.tx) == self.target || Some(event.rx) == self.target {
                out.write_str(" | TARGET")?;
            }
            writeln!(out)?;
        }
        writeln!(
            out,
            "\n- = unavailable. Signal is at scanner; silence does not prove an outage."
        )?;
        writeln!(
            out,
            "Passive capture cannot verify DHCP/DNS/MQTT, true loss or collisions."
        )?;
        writeln!(out, "Raw details: build with WIFI_SCANNER_VERBOSE=1.")?;
        writeln!(
            out,
            "==================== END WI-FI DIAGNOSTICS ==================="
        )
    }
}

struct Value<T>(Option<T>);
impl<T: fmt::Display> fmt::Display for Value<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(value) => fmt::Display::fmt(value, f),
            None => f.pad("-"),
        }
    }
}

struct Percent(u32, u32);
impl fmt::Display for Percent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.1 == 0 {
            return f.write_str("     -");
        }
        let tenths = u64::from(self.0) * 1000 / u64::from(self.1);
        write!(f, "{:>3}.{}%", tenths / 10, tenths % 10)
    }
}

fn security(ap: &Ap) -> &'static str {
    if let Some(rsn) = ap.rsn_details {
        // Only label unambiguous standard AKM sets. Preserve unusual/mixed
        // advertisements as RSN rather than inventing a negotiated mode.
        if rsn.unknown > 0 {
            return "RSN (other)";
        }
        return match rsn.akm {
            0x104 => "WPA2/WPA3",
            0x100 => "WPA3-SAE",
            0x4 => {
                if ap.wpa {
                    "WPA/WPA2"
                } else {
                    "WPA2-PSK"
                }
            }
            0x2 => "Enterprise",
            0x40000 => "OWE",
            _ => "RSN (mixed)",
        };
    }
    if ap.rsn {
        "RSN (?)"
    } else if ap.wpa {
        "WPA"
    } else if ap.ies_incomplete {
        "Unknown"
    } else if ap.capability & 16 != 0 {
        "Privacy (?)"
    } else {
        "Open"
    }
}

fn pmf(ap: &Ap) -> &'static str {
    match ap.rsn_details {
        Some(rsn) if rsn.capabilities & 0x40 != 0 => "req",
        Some(rsn) if rsn.capabilities & 0x80 != 0 => "opt",
        Some(_) => "no",
        None => "-",
    }
}
