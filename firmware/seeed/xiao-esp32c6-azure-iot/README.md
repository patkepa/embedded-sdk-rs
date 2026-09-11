# XIAO ESP32C6 Azure IoT firmware

This is the dedicated, experimental Azure IoT Hub composition target. It is
separate from the general MQTT fixture so Azure policy and credentials do not
leak into generic firmware.

The opt-in development path boots Wi-Fi, registers the ESP32-C6 hardware-random
backend, validates the firmware-owned IoT Hub roots, loads a development X.509
client identity, and composes DNS, TCP, mutual TLS, MQTT 3.1.1, Azure
subscriptions, full twin synchronization, reported properties, telemetry,
direct-method responses, IP-loss cancellation, and bounded reconnect backoff.

The safe-default build contains no device identity and does not connect. The
development identity below is compiled into the image and build artifacts; it
is suitable only for a revocable test-hub identity. Production firmware still
requires protected runtime provisioning or an opaque signer. The current
trust bundle is replaceable through a firmware update; independently
updateable protected trust storage also remains open.

Public development inputs are compiled into the reference image:

```text
WIFI_SSID
WIFI_PASSWORD
AZURE_IOT_HUB_HOSTNAME
AZURE_IOT_DEVICE_ID
AZURE_IOT_AUTH_MODE=development-x509
AZURE_IOT_ALLOW_EMBEDDED_DEVELOPMENT_CREDENTIALS=1
AZURE_IOT_CLIENT_CERTIFICATE_PEM
AZURE_IOT_CLIENT_PRIVATE_KEY_PEM
AZURE_IOT_TRUSTED_UNIX_TIME
```

The timestamp is an integrity-checked Unix-time snapshot supplied by the build
environment and advanced from boot using Embassy's monotonic clock. It must be
current enough for server and client certificate validation. It is not a
replacement for the production persisted/authenticated time design.

The client certificate input currently accepts one PEM leaf certificate. The
private key accepts unencrypted PKCS#8 (`PRIVATE KEY`), PKCS#1
(`RSA PRIVATE KEY`), or SEC1 (`EC PRIVATE KEY`) PEM. Never print either input,
commit it, or use a production identity with this development path.

Build the safe-default image with:

```console
cargo xtask build xiao-esp32c6/azure-iot
```

An opt-in local build can inject a dedicated test identity without copying it
into this repository:

```console
WIFI_SSID='network' WIFI_PASSWORD='passphrase' \
AZURE_IOT_HUB_HOSTNAME='test-hub.azure-devices.net' \
AZURE_IOT_DEVICE_ID='test-device' \
AZURE_IOT_AUTH_MODE='development-x509' \
AZURE_IOT_ALLOW_EMBEDDED_DEVELOPMENT_CREDENTIALS='1' \
AZURE_IOT_CLIENT_CERTIFICATE_PEM="$(<client-cert.pem)" \
AZURE_IOT_CLIENT_PRIVATE_KEY_PEM="$(<client-key.pem)" \
AZURE_IOT_TRUSTED_UNIX_TIME="$(date +%s)" \
  cargo xtask run xiao-esp32c6/azure-iot
```

The reference handler acknowledges cloud delivery but deliberately returns
HTTP-style status `501` for product direct methods and reports an empty property
document. Product-specific method execution and desired/reported property
schemas belong in the consuming firmware.
