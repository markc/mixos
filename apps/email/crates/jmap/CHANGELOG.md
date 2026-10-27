<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased

- Carry ordered References through composed messages, MIME and retry
  fingerprints. Strip CR/LF like the other headers and retain the existing
  In-Reply-To fallback when no explicit References are supplied.
