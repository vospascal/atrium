//! Step 4: one take or a rest. A gap between two events shorter than the
//! bout criterion interval joins them into one take; a longer one is a rest.
//!
//! The criterion is fitted, after Sibly, Nott & Fletcher 1990 (Anim. Behav.
//! 39: 63–69), who took up Slater & Lester 1982: the gaps are drawn by two
//! Poisson processes, a fast one within a bout (here: the pulses or syllables
//! of one take) and a slow one between bouts (the rests). The mixture of two
//! exponential distributions is fitted to the gaps, and the criterion is the
//! gap at which the two processes are equally likely to have drawn it,
//! t = ln(N_f·λ_f / (N_s·λ_s)) / (λ_f − λ_s), Sibly et al.'s criterion that
//! misassigns fewest gaps. The fit is by maximum likelihood (expectation
//! maximisation), as the method's authors refined it (Langton, Collett &
//! Sibly 1995, `diveMove`'s `fitMLEbouts`), not by least squares on a binned
//! log-frequency histogram, whose answer moves with the bin width.
//!
//! Two tests guard against joining what is not two processes:
//!
//! - Two processes must fit better than one for their two extra parameters,
//!   by Schwarz's Bayesian information criterion (Schwarz 1978).
//! - And the fitted distribution of log gaps must have two peaks. Rests are
//!   not Poisson (a frog rests 1.8–5.6 s, not anything from 0 up), and a
//!   mixture of two exponentials fits any gaps that are not exponential
//!   better than one does, so the information criterion alone would split
//!   rests of 1 to 4 s into "short" and "long" and join calls across the
//!   short ones. Two kinds of gap, as the frog's pulses (0.06–0.3 s) and
//!   rests (1–3 s) with little in between, show as a dip between two peaks.
//!
//! If either test fails, nothing is joined, and the summary says why.

use super::events::Found;
use super::record::{BoutCriterion, CriterionDecision, OneProcess, Process, TwoProcesses};

/// The fewest gaps a mixture of two processes (three parameters) is fitted to.
const FEWEST_GAPS: usize = 3;
const ITERATIONS: usize = 2000;
/// The fit has converged when the log-likelihood moves less than this.
const CONVERGED: f64 = 1e-10;

/// The criterion for the gaps between `events`, each `step_seconds` a reading.
pub(super) fn fit(events: &[Found], step_seconds: f64) -> BoutCriterion {
    let gaps: Vec<f64> = events
        .windows(2)
        .map(|pair| (pair[1].first - pair[0].last) as f64 * step_seconds)
        .collect();
    fit_gaps(gaps)
}

pub(super) fn fit_gaps(gaps: Vec<f64>) -> BoutCriterion {
    let count = gaps.len();
    let one = (count > 0).then(|| {
        let rate = count as f64 / gaps.iter().sum::<f64>().max(1e-12);
        let log_likelihood = count as f64 * rate.ln() - rate * gaps.iter().sum::<f64>();
        OneProcess {
            rate,
            log_likelihood,
            bic: (count as f64).ln() - 2.0 * log_likelihood,
        }
    });
    let two = (count >= FEWEST_GAPS).then(|| two_processes(&gaps));
    let (decision, criterion_seconds) = match (one, two) {
        (Some(one), Some(two)) => {
            if two.bic >= one.bic {
                (CriterionDecision::OneProcess, None)
            } else if !two.two_peaks {
                (CriterionDecision::OnePeak, None)
            } else {
                match criterion(&two, count) {
                    Some(seconds) => (CriterionDecision::TwoProcesses, Some(seconds)),
                    None => (CriterionDecision::OnePeak, None),
                }
            }
        }
        _ => (CriterionDecision::TooFewGaps, None),
    };
    BoutCriterion {
        gap_seconds: gaps,
        one,
        two,
        decision,
        criterion_seconds,
    }
}

/// Sibly et al.'s criterion: where N_f·λ_f·e^(−λ_f·t) = N_s·λ_s·e^(−λ_s·t).
fn criterion(two: &TwoProcesses, count: usize) -> Option<f64> {
    let fast = two.fast.share * count as f64 * two.fast.rate;
    let slow = two.slow.share * count as f64 * two.slow.rate;
    let seconds = (fast / slow).ln() / (two.fast.rate - two.slow.rate);
    (seconds.is_finite() && seconds > 0.0).then_some(seconds)
}

/// The two-process mixture of most likelihood, from several starts: the gaps
/// split at each tenth of their sorted order and at their largest step in log.
fn two_processes(gaps: &[f64]) -> TwoProcesses {
    let mut sorted = gaps.to_vec();
    sorted.sort_by(f64::total_cmp);
    let count = sorted.len();
    let mut splits: Vec<usize> = (1..10)
        .map(|tenth| (count * tenth / 10).clamp(1, count - 1))
        .collect();
    let largest_step = (1..count)
        .max_by(|first, second| {
            let step = |index: usize| (sorted[index].max(1e-6) / sorted[index - 1].max(1e-6)).ln();
            step(*first).total_cmp(&step(*second))
        })
        .unwrap_or(1);
    splits.push(largest_step);
    splits.sort_unstable();
    splits.dedup();
    let best = splits
        .into_iter()
        .map(|split| {
            let mean = |part: &[f64]| part.iter().sum::<f64>() / part.len() as f64;
            expectation_maximisation(
                gaps,
                split as f64 / count as f64,
                1.0 / mean(&sorted[..split]).max(1e-6),
                1.0 / mean(&sorted[split..]).max(1e-6),
            )
        })
        .max_by(|first, second| first.3.total_cmp(&second.3))
        .expect("at least one split");
    let (share, fast_rate, slow_rate, log_likelihood) = best;
    let fast = Process {
        rate: fast_rate,
        share,
    };
    let slow = Process {
        rate: slow_rate,
        share: 1.0 - share,
    };
    TwoProcesses {
        fast,
        slow,
        log_likelihood,
        bic: 3.0 * (count as f64).ln() - 2.0 * log_likelihood,
        two_peaks: two_peaks(fast, slow),
    }
}

/// Fitted from a start: the fast process's share, both rates (fast first)
/// and the log-likelihood.
fn expectation_maximisation(
    gaps: &[f64],
    mut share: f64,
    mut fast: f64,
    mut slow: f64,
) -> (f64, f64, f64, f64) {
    let mut previous = f64::NEG_INFINITY;
    let mut log_likelihood = f64::NEG_INFINITY;
    let mut responsibility = vec![0.0f64; gaps.len()];
    for _ in 0..ITERATIONS {
        log_likelihood = 0.0;
        for (gap, weight) in gaps.iter().zip(responsibility.iter_mut()) {
            let from_fast = share.max(1e-300).ln() + fast.ln() - fast * gap;
            let from_slow = (1.0 - share).max(1e-300).ln() + slow.ln() - slow * gap;
            let largest = from_fast.max(from_slow);
            let total = largest + ((from_fast - largest).exp() + (from_slow - largest).exp()).ln();
            *weight = (from_fast - total).exp();
            log_likelihood += total;
        }
        let fast_weight: f64 = responsibility.iter().sum();
        let slow_weight = gaps.len() as f64 - fast_weight;
        let fast_time: f64 = gaps
            .iter()
            .zip(&responsibility)
            .map(|(gap, weight)| gap * weight)
            .sum();
        let slow_time: f64 = gaps.iter().sum::<f64>() - fast_time;
        share = fast_weight / gaps.len() as f64;
        if fast_weight > 1e-9 && fast_time > 0.0 {
            fast = fast_weight / fast_time;
        }
        if slow_weight > 1e-9 && slow_time > 0.0 {
            slow = slow_weight / slow_time;
        }
        if (log_likelihood - previous).abs() < CONVERGED {
            break;
        }
        previous = log_likelihood;
    }
    if fast < slow {
        (1.0 - share, slow, fast, log_likelihood)
    } else {
        (share, fast, slow, log_likelihood)
    }
}

/// Whether the mixture's density of y = ln(gap),
/// Σ share·λ·e^y·exp(−λ·e^y), has two peaks.
fn two_peaks(fast: Process, slow: Process) -> bool {
    let density = |y: f64| {
        [fast, slow]
            .iter()
            .map(|process| {
                let scaled = process.rate * y.exp();
                process.share * scaled * (-scaled).exp()
            })
            .sum::<f64>()
    };
    // Each process peaks at y = −ln λ; look a decade and a half either side.
    let from = (0.03 / fast.rate).ln();
    let to = (30.0 / slow.rate).ln();
    let steps = ((to - from) / 0.01).ceil().max(2.0) as usize;
    let values: Vec<f64> = (0..=steps)
        .map(|step| density(from + (to - from) * step as f64 / steps as f64))
        .collect();
    let peaks = (1..values.len() - 1)
        .filter(|index| values[*index] > values[index - 1] && values[*index] >= values[index + 1])
        .count();
    peaks >= 2
}

/// Events joined into takes: runs of events whose gaps are shorter than the
/// criterion. Each take is its first and last event's index.
pub(super) fn join(events: &[Found], criterion: &BoutCriterion) -> Vec<(usize, usize)> {
    let mut takes: Vec<(usize, usize)> = Vec::new();
    for (index, gap) in std::iter::once(None)
        .chain(criterion.gap_seconds.iter().map(Some))
        .enumerate()
        .take(events.len())
    {
        let joins =
            matches!((gap, criterion.criterion_seconds), (Some(gap), Some(limit)) if *gap < limit);
        match takes.last_mut() {
            Some(last) if joins => last.1 = index,
            _ => takes.push((index, index)),
        }
    }
    takes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `count` gaps at the quantiles of an exponential distribution of mean
    /// `mean` (s): a Poisson process's gaps, without the chance.
    fn exponential(mean: f64, count: usize) -> Vec<f64> {
        (0..count)
            .map(|index| -mean * (1.0 - (index as f64 + 0.5) / count as f64).ln())
            .collect()
    }

    /// Gaps from two Poisson processes, 0.1 s and 5 s apart on average: two
    /// processes, rates within 10 % of those drawn, and the criterion between.
    #[test]
    fn two_processes_are_told_apart() {
        let mut gaps = exponential(0.1, 200);
        gaps.extend(exponential(5.0, 50));
        let fitted = fit_gaps(gaps);
        assert_eq!(
            fitted.decision,
            CriterionDecision::TwoProcesses,
            "{fitted:?}"
        );
        let two = fitted.two.expect("fitted");
        assert!((two.fast.rate / 10.0 - 1.0).abs() < 0.1, "{two:?}");
        assert!((two.slow.rate / 0.2 - 1.0).abs() < 0.1, "{two:?}");
        assert!((two.fast.share - 0.8).abs() < 0.05, "{two:?}");
        let criterion = fitted.criterion_seconds.expect("a criterion");
        assert!(criterion > 0.1 && criterion < 5.0, "{criterion}");
    }

    /// Rests spread evenly from 1 to 4 s are one kind, though two
    /// exponentials fit them better than one.
    #[test]
    fn rests_of_one_kind_join_nothing() {
        let gaps: Vec<f64> = (0..40)
            .map(|index| 1.0 + 3.0 * index as f64 / 39.0)
            .collect();
        let fitted = fit_gaps(gaps);
        assert_eq!(fitted.criterion_seconds, None, "{fitted:?}");
        let events = vec![
            Found {
                first: 0,
                last: 0,
                peak_above_bed: 0.0
            };
            41
        ];
        assert_eq!(join(&events, &fitted).len(), 41);
    }

    #[test]
    fn too_few_gaps_join_nothing() {
        let fitted = fit_gaps(vec![0.1, 3.0]);
        assert_eq!(fitted.decision, CriterionDecision::TooFewGaps);
        assert_eq!(fitted.criterion_seconds, None);
    }
}
