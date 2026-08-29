# Security Policy

## Reporting a vulnerability

Do not disclose suspected vulnerabilities in a public issue. Contact the
repository maintainers privately through GitHub's security advisory reporting
flow. Include affected versions, hardware, reproduction steps, impact, and any
suggested mitigation.

## Supported versions

The project is currently pre-1.0 and does not yet publish production support
windows. Security fixes are applied to the default branch. A versioned support
table will be added with the first supported SDK release.

## Security baseline

- Production secrets must not be committed or compiled into source-controlled
  configuration.
- Firmware update and provisioning formats must be versioned and threat-modeled.
- Cryptographic implementations should come from reviewed libraries or hardware
  accelerators, not local ad-hoc algorithms.
- Unsafe Rust and native FFI require isolated boundaries and documented safety
  invariants.

