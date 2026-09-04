# XIAO ESP32C6 Azure IoT firmware

This is the dedicated, experimental Azure IoT Hub composition target. It is
separate from the general MQTT fixture so Azure policy and credentials do not
leak into generic firmware.

The current slice boots Wi-Fi, registers the ESP32-C6 hardware-random backend,
owns fixed MQTT and telemetry-queue storage, validates public Azure identity
configuration, resolves the hub, and fails closed before authentication. A
live connection remains disabled until the firmware has a trusted-time source,
an updateable trust bundle, and a runtime credential source.

Public development inputs are compiled into the reference image:

```text
WIFI_SSID
WIFI_PASSWORD
AZURE_IOT_HUB_HOSTNAME
AZURE_IOT_DEVICE_ID
AZURE_IOT_AUTH_MODE=runtime-sas
```

Do not pass an IoT Hub connection string, symmetric key, or reusable SAS token
through these inputs. Secrets will be accepted only through the future runtime
credential boundary and must never be printed.

Build the safe-default image with:

```console
cargo xtask build-xiao-esp32c6-azure-iot
```
