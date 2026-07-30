//! Luby Transform (LT) fountain codes — encode / peel-decode.

use std::collections::{HashMap, HashSet, VecDeque};

/// xorshift64* — deterministic PRNG seeded per (file_id, esi).
#[derive(Clone)]
pub struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    pub fn new(seed: u64) -> Self {
        let state = if seed == 0 { 0x9E3779B97F4A7C15 } else { seed };
        Self { state }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    pub fn gen_range(&mut self, max: u32) -> u32 {
        if max == 0 {
            return 0;
        }
        self.next_u32() % max
    }
}

/// Robust Soliton degree distribution (Luby).
pub fn robust_soliton_degree(rng: &mut XorShift64, k: u32) -> u32 {
    if k <= 1 {
        return 1;
    }
    let k_f = k as f64;
    let c = 0.1;
    let delta = 0.5;
    let r = c * (k_f / delta).ln() * k_f.sqrt();
    let r = r.max(1.0);
    let mut tau = vec![0.0f64; (k + 1) as usize];
    let mut rho = vec![0.0f64; (k + 1) as usize];
    let bound = (k_f / r).floor() as u32;

    for d in 1..=k {
        if d == 1 {
            rho[d as usize] = 1.0 / k_f;
        } else {
            rho[d as usize] = 1.0 / (d as f64 * (d as f64 - 1.0));
        }
    }
    for d in 1..=k {
        if d < bound {
            tau[d as usize] = r / (d as f64 * k_f);
        } else if d == bound {
            tau[d as usize] = (r * (r / delta).ln()) / k_f;
        }
    }
    let mut z = 0.0;
    let mut mu = vec![0.0f64; (k + 1) as usize];
    for d in 1..=k {
        mu[d as usize] = rho[d as usize] + tau[d as usize];
        z += mu[d as usize];
    }
    for d in 1..=k {
        mu[d as usize] /= z;
    }
    // Inverse transform sample
    let u = (rng.next_u64() as f64) / (u64::MAX as f64);
    let mut acc = 0.0;
    for d in 1..=k {
        acc += mu[d as usize];
        if u <= acc {
            return d;
        }
    }
    1
}

pub fn neighbors(file_id: u64, esi: u32, k: u32) -> Vec<u32> {
    let seed = file_id
        .wrapping_mul(0x9E3779B97F4A7C15)
        .wrapping_add(esi as u64)
        .wrapping_mul(0xBF58476D1CE4E5B9);
    let mut rng = XorShift64::new(seed);
    let degree = robust_soliton_degree(&mut rng, k).min(k).max(1);
    let mut chosen = HashSet::with_capacity(degree as usize);
    while chosen.len() < degree as usize {
        chosen.insert(rng.gen_range(k));
    }
    let mut v: Vec<u32> = chosen.into_iter().collect();
    v.sort_unstable();
    v
}

pub fn xor_into(dst: &mut [u8], src: &[u8]) {
    let n = dst.len().min(src.len());
    for i in 0..n {
        dst[i] ^= src[i];
    }
}

pub struct Encoder {
    pub file_id: u64,
    pub k: u32,
    pub symbol_size: usize,
    symbols: Vec<Vec<u8>>,
    next_esi: u32,
}

impl Encoder {
    pub fn new(file_id: u64, data: &[u8], symbol_size: usize) -> Self {
        let symbol_size = symbol_size.max(1);
        let mut symbols = Vec::new();
        let mut offset = 0;
        while offset < data.len() || symbols.is_empty() {
            let end = (offset + symbol_size).min(data.len());
            let mut sym = vec![0u8; symbol_size];
            if offset < data.len() {
                sym[..end - offset].copy_from_slice(&data[offset..end]);
            }
            symbols.push(sym);
            offset = end;
            if offset >= data.len() {
                break;
            }
        }
        // Ensure at least one symbol
        if symbols.is_empty() {
            symbols.push(vec![0u8; symbol_size]);
        }
        Self {
            file_id,
            k: symbols.len() as u32,
            symbol_size,
            symbols,
            next_esi: 0,
        }
    }

    pub fn encode_next(&mut self) -> (u32, Vec<u8>) {
        let esi = self.next_esi;
        self.next_esi = self.next_esi.wrapping_add(1);
        let ns = neighbors(self.file_id, esi, self.k);
        let mut out = vec![0u8; self.symbol_size];
        for &i in &ns {
            xor_into(&mut out, &self.symbols[i as usize]);
        }
        (esi, out)
    }
}

#[derive(Debug, Clone)]
struct Coded {
    neighbors: Vec<u32>,
    data: Vec<u8>,
}

pub struct Decoder {
    pub file_id: u64,
    pub k: u32,
    pub symbol_size: usize,
    pub total_len: u64,
    recovered: Vec<Option<Vec<u8>>>,
    /// esi -> coded packet (unresolved)
    pending: HashMap<u32, Coded>,
    /// source index -> list of esi that still reference it
    index: HashMap<u32, HashSet<u32>>,
    seen_esi: HashSet<u32>,
    pub new_count: u64,
    pub dup_count: u64,
    pub red_count: u64,
}

impl Decoder {
    pub fn new(file_id: u64, k: u32, symbol_size: usize, total_len: u64) -> Self {
        Self {
            file_id,
            k,
            symbol_size,
            total_len,
            recovered: vec![None; k as usize],
            pending: HashMap::new(),
            index: HashMap::new(),
            seen_esi: HashSet::new(),
            new_count: 0,
            dup_count: 0,
            red_count: 0,
        }
    }

    pub fn recovered_count(&self) -> u32 {
        self.recovered.iter().filter(|s| s.is_some()).count() as u32
    }

    pub fn is_complete(&self) -> bool {
        self.recovered_count() == self.k
    }

    /// Returns: 0=dup, 1=red, 2=new
pub fn ingest(&mut self, esi: u32, payload: &[u8]) -> u8 {
        if self.seen_esi.contains(&esi) {
            self.dup_count += 1;
            return 0;
        }
        self.seen_esi.insert(esi);

        if self.is_complete() {
            self.red_count += 1;
            return 1;
        }

        let mut ns = neighbors(self.file_id, esi, self.k);
        let mut data = vec![0u8; self.symbol_size];
        let n = data.len().min(payload.len());
        data[..n].copy_from_slice(&payload[..n]);

        // Peel already-recovered neighbors
        let mut still = Vec::new();
        for &i in &ns {
            if let Some(ref known) = self.recovered[i as usize] {
                xor_into(&mut data, known);
            } else {
                still.push(i);
            }
        }
        ns = still;

        if ns.is_empty() {
            self.red_count += 1;
            return 1;
        }

        self.new_count += 1;
        self.pending.insert(
            esi,
            Coded {
                neighbors: ns.clone(),
                data,
            },
        );
        for &i in &ns {
            self.index.entry(i).or_default().insert(esi);
        }

        self.peel();
        2
    }

    fn peel(&mut self) {
        let mut queue: VecDeque<u32> = VecDeque::new();
        for (&esi, coded) in &self.pending {
            if coded.neighbors.len() == 1 {
                queue.push_back(esi);
            }
        }

        while let Some(esi) = queue.pop_front() {
            let Some(coded) = self.pending.remove(&esi) else {
                continue;
            };
            if coded.neighbors.len() != 1 {
                // Re-check; may have been updated
                if coded.neighbors.len() == 1 {
                    // fall through
                } else if coded.neighbors.is_empty() {
                    continue;
                } else {
                    self.pending.insert(esi, coded);
                    continue;
                }
            }
            let idx = coded.neighbors[0];
            if self.recovered[idx as usize].is_some() {
                continue;
            }
            let symbol = coded.data.clone();
            self.recovered[idx as usize] = Some(symbol.clone());

            // XOR into all pending packets that reference idx
            let affected: Vec<u32> = self
                .index
                .get(&idx)
                .map(|s| s.iter().copied().collect())
                .unwrap_or_default();
            self.index.remove(&idx);

            for a_esi in affected {
                if a_esi == esi {
                    continue;
                }
                if let Some(pkt) = self.pending.get_mut(&a_esi) {
                    if let Some(pos) = pkt.neighbors.iter().position(|&x| x == idx) {
                        pkt.neighbors.swap_remove(pos);
                        xor_into(&mut pkt.data, &symbol);
                        if pkt.neighbors.len() == 1 {
                            queue.push_back(a_esi);
                        } else if pkt.neighbors.is_empty() {
                            // inconsistent / redundant
                            self.pending.remove(&a_esi);
                        }
                    }
                }
            }
        }
    }

    pub fn assemble(&self) -> Option<Vec<u8>> {
        if !self.is_complete() {
            return None;
        }
        let mut out = Vec::with_capacity(self.total_len as usize);
        for sym in &self.recovered {
            out.extend_from_slice(sym.as_ref()?);
        }
        out.truncate(self.total_len as usize);
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_small() {
        let data: Vec<u8> = (0u8..200).collect();
        let file_id = 0xDEADBEEFCAFEBABE;
        let mut enc = Encoder::new(file_id, &data, 32);
        let mut dec = Decoder::new(file_id, enc.k, enc.symbol_size, data.len() as u64);
        let mut n = 0;
        while !dec.is_complete() && n < enc.k * 20 {
            let (esi, payload) = enc.encode_next();
            dec.ingest(esi, &payload);
            n += 1;
        }
        assert!(dec.is_complete(), "failed after {n} symbols, recovered {}", dec.recovered_count());
        assert_eq!(dec.assemble().unwrap(), data);
    }
}
