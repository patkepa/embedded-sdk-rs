# Azure IoT Hub server trust roots

The reference firmware carries the two public-cloud roots named by Microsoft
for IoT Hub server authentication as of 2026-09-04. These are roots, not Azure
leaf or intermediate certificate pins.

| File | Official source | Published SHA-1 fingerprint | Verified SHA-256 fingerprint |
| --- | --- | --- | --- |
| `digicert-global-root-g2.pem` | [DigiCert](https://cacerts.digicert.com/DigiCertGlobalRootG2.crt) | `DF3C24F9BFD666761B268073FE06D1CC8D4F82A4` | `CB3CCBB76031E5E0138F8DD39A23F9DE47FFC35E43C1144CEA27D46A5AB1CB5F` |
| `microsoft-rsa-root-2017.pem` | [Microsoft](https://www.microsoft.com/pkiops/certs/Microsoft%20RSA%20Root%20Certificate%20Authority%202017.crt) | `73A5E64A3BFF8316FF0EDCCC618A906E4EAE4D74` | `C741F70F4B2A8D88BF2E71C14122EF53EF10EBA0CFA5E64CFA20F418853073E0` |

The authoritative service policy is Microsoft Learn's
[Azure IoT Hub TLS support](https://learn.microsoft.com/en-us/azure/iot-hub/iot-hub-tls-support)
page. Review that page and revalidate certificate fingerprints before changing
this bundle. Replacing the bundle currently requires a signed firmware update;
an independently updatable protected trust store is still a production gap.
