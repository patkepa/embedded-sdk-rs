# Beetle ESP32-C6 Wi-Fi scanner HIL

Status: **not run**. Requires a DFR1117 Beetle, USB host, controlled 2.4 GHz AP,
an IoT client, and ideally an independent monitor capture for comparison.

1. Build and flash `beetle-esp32c6/wifi-scanner`. Confirm board boot metadata,
   serial output, channel sequence 1–13, five-second dwell, and reported gaps.
2. Advertise known visible/hidden SSIDs on channels 1, 6, 11. Compare BSSID,
   SSID, channel, RSN/PMF, DTIM and beacon interval against AP configuration.
   Separately validate channel locks on 12/13 where supported by the fixture.
3. Lock to the AP channel and configure the client MAC as target. Generate
   traffic. Compare header counts/retry fraction with an independent sniffer,
   allowing for receiver-dependent losses. Verify TX RSSI belongs to the sender
   and addressed frames alone do not populate receiver RSSI.
4. Reconnect the client and restart the AP normally. Correlate association,
   disconnect and rejection codes with AP/client logs. For PMF-protected
   management traffic verify no encrypted bytes appear as a reason code.
5. Power down or put the client to sleep. Verify stale age and zero window
   samples, with no fabricated outage, packet loss, or zero-sample retry rate.
6. Increase traffic and identity count. Confirm queue-drop/table-eviction/event
   overflow accounting, target retention, bounded memory and no callback stalls.
   Measure report duration and blind intervals at full table capacity.
7. Run for at least 24 hours. Check timestamp wrap handling, heap stability,
   watchdog resets, USB stalls and correct report/window separation.

Record board revision, firmware commit, AP/client versions, configuration,
serial capture, independent capture if available, and pass/fail for every step.
