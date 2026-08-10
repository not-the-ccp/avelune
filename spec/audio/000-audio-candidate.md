> HISTORICAL / NON-NORMATIVE: superseded by the Draft Generation 1 `001-*` specification.

# Avelune Audio Candidate 0.0

Status: **EXPERIMENTAL / NOT FROZEN**

## Intended architecture

The main research direction is a general-purpose lapped transform codec with explicit band energies and optional short-term/pitch whitening before the common transform path. It is not frozen.

Initial target configuration is 48 kHz mono/stereo with nominal 20 ms framing and shorter transform partitions for transients.

## Experiment 0: lossless predictive PCM

The executable bring-up format deliberately starts with a simple *custom* codec rather than embedding an existing codec or treating PCM as the final design.

Input samples are signed 16-bit integers. For each channel independently:

1. the first sample is predicted as zero;
2. subsequent samples are predicted by the immediately previous reconstructed sample for the same channel;
3. the signed residual is mapped by ZigZag coding;
4. the unsigned value is encoded as bounded LEB128.

Predictor state MUST reset at every epoch boundary.

This experiment is useful for packetization, synchronization, truncation handling and exact round-trip tests. It is not the intended lossy audio architecture.
