# Experimental gates

Each experiment must state a falsifiable hypothesis, corpus, metrics, result and decision.

Planned gates:

1. range coder vs 1/2/4-way rANS;
2. block MV vs global+delta vs coarse motion lattice+override;
3. no filter vs deblock vs single direction-aware reconstruction filter;
4. palette+IBC vs epoch patch dictionary on screen content;
5. transform-only audio vs optional LPC/pitch whitening;
6. 1/2/4/8 s epoch interval tradeoff;
7. scalar vs WASM SIMD decoder kernels;
8. constrained partition grammar vs expanded grammar rate/complexity value.
