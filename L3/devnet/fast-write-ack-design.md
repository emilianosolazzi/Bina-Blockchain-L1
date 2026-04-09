# Temporal Gradient L3 — Fast Write Acknowledgement Design

Status: Draft v0  
Date: March 25, 2026

---

## 1. Purpose

This document defines how the first L3 should present fast writes without confusing acknowledgement with finality.

---

## 2. Core rule

A fast write should move through explicit states.

Recommended state model:

1. `received`
2. `accepted`
3. `sequenced`
4. `settled`
5. `finalized`

Not every user interface needs to show all five, but the backend should preserve them.

---

## 3. Why this model matters

If the chain returns only “success” or “failed,” consumers cannot distinguish:

- accepted but not yet durable,
- durable but not finalized,
- finalized but not yet materialized into read models.

That ambiguity destroys trust.

---

## 4. Recommended UX model

The user-facing product should behave like this:

- respond quickly once the write is accepted,
- return a write ID / transaction reference,
- expose current write state,
- update asynchronously as the write advances.

This gives web2-like responsiveness without misrepresenting settlement.

---

## 5. Recommended response shape

Suggested acknowledgement payload:

```json
{
  "status": "accepted",
  "writeId": "uuid-or-hash",
  "txHash": "0x...",
  "acceptedAt": 1774444800000,
  "nextState": "sequenced"
}
```

Suggested status payload:

```json
{
  "writeId": "uuid-or-hash",
  "status": "sequenced",
  "acceptedAt": 1774444800000,
  "updatedAt": 1774444800100,
  "txHash": "0x...",
  "materialized": true,
  "finalized": false
}
```

---

## 6. Read-model integration

The write-ack model should connect directly to the materialized store.

That means:

- once a write is accepted, it can be tracked,
- once sequenced or settled, the read model should be updated,
- once finalized, trust labels and final state should reflect that.

---

## 7. Randomness-specific use

For randomness-related writes:

- epoch commit requests can return `accepted` quickly,
- proof purchases can return `accepted` with a receipt ID,
- certificate issuance can return `accepted` with a certificate reference,
- higher assurance labels should appear only after the right state transitions complete.

---

## 8. Failure model

The system should expose explicit failure classes:

- rejected before acceptance,
- accepted but dropped,
- sequenced but not yet materialized,
- materialized but later invalidated,
- delayed finalization.

This is better than silent retries.

---

## 9. Performance alignment

This model supports the performance goals because it lets the chain optimize for:

- fast acknowledgements,
- asynchronous finality,
- fast reads from the materialized store,
- honest status transitions.

---

## 10. Summary

The right way to feel faster than web2 is not fake finality.

It is:

- fast acknowledgements,
- strong status visibility,
- quick read-model updates,
- and honest eventual finality.