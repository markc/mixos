---
title: test-broker
description: A development fixture that runs the real noded modules in an isolated broker runtime for the Term-to-Mix native session acceptance tests.
---

# test-broker

Development fixture for the Term-to-Mix native session acceptance tests.
It includes the actual [noded](../noded/README.md) modules from their owning
service and a copy of Term's sealed-FD implementation
(`src/session_fd.rs`, from term-core; it returns to term-core when Term is
ported to egui). It has no broker copy or simulated session transport. The
fixture controls an isolated broker runtime, verified Unix ingress, restart
and pause boundaries.

The source is [`libs/test-broker`](https://github.com/markc/mixos/tree/main/libs/test-broker).
