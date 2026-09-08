use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageBand {
    None,
    Partial,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryBand {
    Unavailable,
    Under50Mib,
    From50ToUnder100Mib,
    From100ToUnder250Mib,
    From250ToUnder500Mib,
    From500ToUnder1024Mib,
    From1ToUnder2Gib,
    From2ToUnder4Gib,
    AtLeast4Gib,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuBand {
    Unavailable,
    Under1Percent,
    From1ToUnder5Percent,
    From5ToUnder10Percent,
    From10ToUnder25Percent,
    From25ToUnder50Percent,
    From50ToUnder100Percent,
    From100ToUnder200Percent,
    AtLeast200Percent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IoRateBand {
    Unavailable,
    Zero,
    Under1KibPerSecond,
    From1ToUnder10KibPerSecond,
    From10ToUnder100KibPerSecond,
    From100ToUnder1024KibPerSecond,
    From1ToUnder10MibPerSecond,
    AtLeast10MibPerSecond,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceUsageSummary {
    pub memory_mean: MemoryBand,
    pub memory_max: MemoryBand,
    pub memory_coverage: CoverageBand,
    pub cpu_average: CpuBand,
    pub cpu_coverage: CoverageBand,
    pub read_rate_average: IoRateBand,
    pub read_coverage: CoverageBand,
    pub write_rate_average: IoRateBand,
    pub write_coverage: CoverageBand,
    pub database_size: MemoryBand,
    pub database_coverage: CoverageBand,
    pub wal_size: MemoryBand,
    pub wal_coverage: CoverageBand,
}

pub(crate) fn coverage(valid: u64, opportunities: u64) -> CoverageBand {
    if valid == 0 {
        CoverageBand::None
    } else if valid == opportunities {
        CoverageBand::Full
    } else {
        CoverageBand::Partial
    }
}

pub(crate) fn memory_band(bytes: u128) -> MemoryBand {
    const MIB: u128 = 1024 * 1024;
    const GIB: u128 = 1024 * MIB;

    match bytes {
        value if value < 50 * MIB => MemoryBand::Under50Mib,
        value if value < 100 * MIB => MemoryBand::From50ToUnder100Mib,
        value if value < 250 * MIB => MemoryBand::From100ToUnder250Mib,
        value if value < 500 * MIB => MemoryBand::From250ToUnder500Mib,
        value if value < GIB => MemoryBand::From500ToUnder1024Mib,
        value if value < 2 * GIB => MemoryBand::From1ToUnder2Gib,
        value if value < 4 * GIB => MemoryBand::From2ToUnder4Gib,
        _ => MemoryBand::AtLeast4Gib,
    }
}

pub(crate) fn cpu_band(cpu_ns: u128, elapsed_ns: u128) -> CpuBand {
    ratio_band(
        cpu_ns,
        elapsed_ns,
        &[
            CpuBand::Under1Percent,
            CpuBand::From1ToUnder5Percent,
            CpuBand::From5ToUnder10Percent,
            CpuBand::From10ToUnder25Percent,
            CpuBand::From25ToUnder50Percent,
            CpuBand::From50ToUnder100Percent,
            CpuBand::From100ToUnder200Percent,
        ],
        &[1, 5, 10, 25, 50, 100, 200],
        100,
        CpuBand::AtLeast200Percent,
    )
}

pub(crate) fn io_rate_band(bytes: u128, elapsed_ns: u128) -> IoRateBand {
    if bytes == 0 {
        return IoRateBand::Zero;
    }
    ratio_band(
        bytes,
        elapsed_ns,
        &[
            IoRateBand::Under1KibPerSecond,
            IoRateBand::From1ToUnder10KibPerSecond,
            IoRateBand::From10ToUnder100KibPerSecond,
            IoRateBand::From100ToUnder1024KibPerSecond,
            IoRateBand::From1ToUnder10MibPerSecond,
        ],
        &[1024, 10 * 1024, 100 * 1024, 1024 * 1024, 10 * 1024 * 1024],
        1_000_000_000,
        IoRateBand::AtLeast10MibPerSecond,
    )
}

fn ratio_band<T: Copy>(
    numerator: u128,
    denominator: u128,
    bands: &[T],
    boundaries: &[u128],
    scale: u128,
    highest: T,
) -> T {
    for (band, boundary) in bands.iter().zip(boundaries) {
        if numerator.saturating_mul(scale) < denominator.saturating_mul(*boundary) {
            return *band;
        }
    }
    highest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_boundaries_are_stable() {
        const MIB: u128 = 1024 * 1024;
        assert_eq!(memory_band(50 * MIB - 1), MemoryBand::Under50Mib);
        assert_eq!(memory_band(50 * MIB), MemoryBand::From50ToUnder100Mib);
        assert_eq!(memory_band(1024 * MIB), MemoryBand::From1ToUnder2Gib);
        assert_eq!(memory_band(4 * 1024 * MIB), MemoryBand::AtLeast4Gib);
    }

    #[test]
    fn cpu_boundaries_allow_more_than_one_core() {
        assert_eq!(cpu_band(99, 100), CpuBand::From50ToUnder100Percent);
        assert_eq!(cpu_band(1, 1), CpuBand::From100ToUnder200Percent);
        assert_eq!(cpu_band(2, 1), CpuBand::AtLeast200Percent);
    }

    #[test]
    fn io_distinguishes_zero_from_unavailable() {
        assert_eq!(io_rate_band(0, 1), IoRateBand::Zero);
        assert_eq!(
            io_rate_band(1023, 1_000_000_000),
            IoRateBand::Under1KibPerSecond
        );
        assert_eq!(
            io_rate_band(1024, 1_000_000_000),
            IoRateBand::From1ToUnder10KibPerSecond
        );
    }

    #[test]
    fn coverage_distinguishes_missing_and_partial_data() {
        assert_eq!(coverage(0, 4), CoverageBand::None);
        assert_eq!(coverage(2, 4), CoverageBand::Partial);
        assert_eq!(coverage(4, 4), CoverageBand::Full);
    }
}
