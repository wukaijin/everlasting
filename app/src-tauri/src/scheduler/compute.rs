//! F2 定时任务的调度计算纯函数(`08-28-f2-scheduled-tasks` design §3,
//! F2b 扩展三档位:`08-28-f2b-schedule-extension` prd D7)。
//!
//! schedule 是 preset 档位(prd D2):`daily HH:MM` / `interval 每 N 分钟` /
//! `weekly 周X HH:MM` / `hourly 每小时第 N 分` / `weekdays 工作日 HH:MM` /
//! `monthly 每月 D 号 HH:MM`,以 internally-tagged JSON 存
//! `scheduled_tasks.schedule` 列。本模块只做「给定 schedule 与时间窗,算出
//! 到期点」的纯计算 —— 无 DB、无 IO、无锁,全部本地时区(chrono `Local`)。
//!
//! 两个核心纯函数(design §3):
//! - [`most_recent_due`]:从 now 向后步进找**最近**到期点
//!   `d`,要求 `not_before < d <= now`;无则 `None`。调度器每 tick 对每个
//!   enabled 任务调用它 —— 命中即「存在未消费的到期点」,catch-up(D4)与
//!   常规触发因此是**同一算法**(无独立 catch-up pass,不存在同 tick 双 fire)。
//! - [`next_fire_display`]:严格 `> from` 的下一个到期点,仅供 UI 列展示
//!   与存库 `next_fire_at`(**不参与触发判定**,触发判定每 tick 重算)。
//!
//! 不变量(单测锁定,见文末 tests):
//! 1. `most_recent_due` 结果恒 `> not_before` 且 `<= now`;
//! 2. `next_fire_display > from`;
//! 3. 两函数对同一 schedule 互相一致(`most_recent_due(s, y, x) == Some(y)`
//!    其中 `y = next_fire_display(s, x)`);
//! 4. **interval 无累积漂移**:连续模拟 fire(含 tick 量化抖动)且落账恒记
//!    理论到期点 `last_fired_at = due` 时,相邻 due 间隔恒等步长。

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Timelike, Weekday};
use serde::{Deserialize, Serialize};

/// schedule preset 档位(prd D2,design §3)。internally tagged JSON:
///
/// ```json
/// { "kind": "daily",    "at": "09:00" }
/// { "kind": "interval", "every_min": 30 }
/// { "kind": "weekly",   "weekday": "mon", "at": "09:00" }
/// { "kind": "hourly",   "minute": 30 }
/// { "kind": "weekdays", "at": "09:00" }
/// { "kind": "monthly",  "day": 15, "at": "09:00" }
/// ```
///
/// 后续档位 additive 扩展(未知 `kind` 反序列化失败 → 入库/更新时被
/// [`parse_schedule`] 拒绝,存量行不受影响)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleSpec {
    /// 每天 `at`(HH:MM 本地时间)。
    Daily { at: String },
    /// 每 `every_min` 分钟一次;网格锚定 `not_before`(=
    /// `max(created_at, last_fired_at)`),落账恒记理论到期点,网格不漂移。
    /// 「每 N 小时/天/周」是前端单位换算(F2b),存库仍是分钟数。
    Interval { every_min: u32 },
    /// 每周 `weekday` 的 `at`(本地时间)。
    Weekly { weekday: Weekday, at: String },
    /// 每小时第 `minute` 分钟(本地时间)。
    Hourly { minute: u32 },
    /// 每工作日(周一至五,无节假日日历)的 `at`(本地时间)。
    Weekdays { at: String },
    /// 每月 `day` 号的 `at`(本地时间)。短月无该日(如 2 月无 31 号)→
    /// **跳过该月**(F2b prd D7,cron 语义,与 DST 跳过防御一致)。
    Monthly { day: u32, at: String },
}

/// 解析并校验 schedule JSON(`scheduled_tasks.schedule` 列的唯一合法入口)。
/// 反序列化失败 / 档位字段非法(HH:MM 越界、`every_min = 0`)都返回
/// 中文错误信息,调用方直接展示。
pub fn parse_schedule(json: &str) -> Result<ScheduleSpec, String> {
    let spec: ScheduleSpec =
        serde_json::from_str(json).map_err(|e| format!("schedule 格式非法: {e}"))?;
    validate_schedule(&spec)?;
    Ok(spec)
}

/// 校验已反序列化的档位字段。`HH:MM` 必须是合法的 24 小时时刻,
/// `every_min` 必须为正(0 会让网格退化成 `not_before` 本身,永判定为
/// 已消费,等价于任务死掉)。
pub fn validate_schedule(spec: &ScheduleSpec) -> Result<(), String> {
    match spec {
        ScheduleSpec::Daily { at }
        | ScheduleSpec::Weekly { at, .. }
        | ScheduleSpec::Weekdays { at } => parse_hh_mm(at).map(|_| ()),
        ScheduleSpec::Monthly { day, at } => {
            if !(1..=31).contains(day) {
                return Err(format!("day 必须在 1-31 之间,得到 {day}"));
            }
            parse_hh_mm(at).map(|_| ())
        }
        ScheduleSpec::Hourly { minute } => {
            if *minute >= 60 {
                Err(format!("minute 必须在 0-59 之间,得到 {minute}"))
            } else {
                Ok(())
            }
        }
        ScheduleSpec::Interval { every_min } => {
            if *every_min == 0 {
                Err("every_min 必须为正整数".to_string())
            } else {
                Ok(())
            }
        }
    }
}

/// 解析 `HH:MM` 为 `(hour, minute)`(24 小时制)。只接受 `H:MM` / `HH:MM`
/// 两种形状(分钟必须两位),越界即拒。
fn parse_hh_mm(s: &str) -> Result<(u32, u32), String> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return Err(format!("时刻必须是 HH:MM 格式,得到「{s}」"));
    }
    let hour: u32 = parts[0]
        .trim()
        .parse()
        .map_err(|_| format!("小时不是数字:「{}」", parts[0]))?;
    let minute: u32 = parts[1]
        .trim()
        .parse()
        .map_err(|_| format!("分钟不是数字:「{}」", parts[1]))?;
    if hour >= 24 {
        return Err(format!("小时必须在 0-23 之间,得到 {hour}"));
    }
    if minute >= 60 {
        return Err(format!("分钟必须在 0-59 之间,得到 {minute}"));
    }
    Ok((hour, minute))
}

/// epoch ms → 本地时间。epoch 时间戳在任意时区下都唯一对应一个时刻,
/// `earliest()` 只是防御性拆包(LocalResult 对合法 epoch 恒为 Single)。
pub(crate) fn ms_to_local(ms: i64) -> DateTime<Local> {
    Local
        .timestamp_millis_opt(ms)
        .earliest()
        .unwrap_or_else(|| Local.timestamp_opt(0, 0).earliest().expect("epoch 0 valid"))
}

/// epoch ms → `YYYY-MM-DD HH:MM`(本地时间)。注入注脚的展示格式
/// (design §4.3,给模型带日期上下文)。
pub(crate) fn format_local_hhmm(ms: i64) -> String {
    use chrono::Timelike;
    let dt = ms_to_local(ms);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute()
    )
}

/// 本地日 + 时分 → 本地时刻。DST 边界防御:
/// - 春令时跳过的本地时刻(不存在)→ `None`,调用方跳过该候选日;
/// - 秋令时回拨的重复时刻 → 取较早者(与「上一周期」直觉一致)。
fn local_at(date: NaiveDate, hour: u32, minute: u32) -> Option<DateTime<Local>> {
    let naive = date.and_hms_opt(hour, minute, 0)?;
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => Some(dt),
        chrono::LocalResult::Ambiguous(earliest, _) => Some(earliest),
        chrono::LocalResult::None => None,
    }
}

/// `base` 所在月偏移 `delta` 个月后的 `(year, month)`(负数 = 往前)。
/// 只做月粒度换算,不判断「日」在该月是否合法(由调用方
/// `NaiveDate::from_ymd_opt` 判定,无效即跳过该月)。
fn month_shift(base: NaiveDate, delta: i32) -> (i32, u32) {
    let total = base.year() * 12 + base.month() as i32 - 1 + delta;
    (total.div_euclid(12), total.rem_euclid(12) as u32 + 1)
}

/// 从 now 向后步进找最近的**未消费**到期点 `d`
/// (`not_before < d <= now`),无则 `None`。
///
/// 各档位的回看窗口(design §3):daily 最多 1 天、weekly 7 天、interval
/// 按 `not_before` 网格逆推一步 —— 因此 `not_before` 之前的多次错过不会
/// 被逐个补跑,D4「补一次、不追多次」由「最近一个到期点」语义天然保证。
/// `not_before = max(created_at, last_fired_at)`;落账恒记 `last_fired_at =
/// due`(理论到期点),同一窗口重复评估时 due 点被 `> not_before` 排除,
/// 幂等不双 fire。
pub fn most_recent_due(schedule: &ScheduleSpec, now_ms: i64, not_before: i64) -> Option<i64> {
    if now_ms <= not_before {
        return None;
    }
    let now = ms_to_local(now_ms);
    match schedule {
        ScheduleSpec::Interval { every_min } => {
            let step = (*every_min).max(1) as i64 * 60_000;
            // 网格锚定 not_before:due_j = not_before + j*step(j >= 1)。
            // 最大满足 `<= now` 的 j 即最近未消费点。
            let j = (now_ms - not_before) / step;
            if j < 1 {
                return None;
            }
            Some(not_before + j * step)
        }
        ScheduleSpec::Daily { at } => {
            let (hour, minute) = parse_hh_mm(at).ok()?;
            for days_back in 0..=1 {
                let date = (now - Duration::days(days_back)).date_naive();
                if let Some(cand) = local_at(date, hour, minute) {
                    let cand_ms = cand.timestamp_millis();
                    if cand_ms > not_before && cand_ms <= now_ms {
                        return Some(cand_ms);
                    }
                }
            }
            None
        }
        ScheduleSpec::Weekly { weekday, at } => {
            let (hour, minute) = parse_hh_mm(at).ok()?;
            // 往回 0..=7 天必含目标周各恰一次(首尾同 weekday 时两次);
            // 从最近的一天起找,首个满足约束的即最近未消费点。
            for days_back in 0..=7 {
                let date = (now - Duration::days(days_back)).date_naive();
                if date.weekday() != *weekday {
                    continue;
                }
                if let Some(cand) = local_at(date, hour, minute) {
                    let cand_ms = cand.timestamp_millis();
                    if cand_ms > not_before && cand_ms <= now_ms {
                        return Some(cand_ms);
                    }
                }
            }
            None
        }
        ScheduleSpec::Hourly { minute } => {
            // 回看 0..=2 个「墙上钟点」:本小时未到点时上一个小时必命中;
            // 第 2 个是为 DST 春令时跳过时刻留的余量(候选经 local_at
            // 判 None 即跳过该钟点)。
            for hours_back in 0..=2 {
                let dt = now - Duration::hours(hours_back);
                if let Some(cand) = local_at(dt.date_naive(), dt.hour(), *minute) {
                    let cand_ms = cand.timestamp_millis();
                    if cand_ms > not_before && cand_ms <= now_ms {
                        return Some(cand_ms);
                    }
                }
            }
            None
        }
        ScheduleSpec::Weekdays { at } => {
            let (hour, minute) = parse_hh_mm(at).ok()?;
            // 回看 0..=3 天:周一最远的候选是周五(周六/周日跳过)。
            for days_back in 0..=3 {
                let date = (now - Duration::days(days_back)).date_naive();
                if matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
                    continue;
                }
                if let Some(cand) = local_at(date, hour, minute) {
                    let cand_ms = cand.timestamp_millis();
                    if cand_ms > not_before && cand_ms <= now_ms {
                        return Some(cand_ms);
                    }
                }
            }
            None
        }
        ScheduleSpec::Monthly { day, at } => {
            let (hour, minute) = parse_hh_mm(at).ok()?;
            let now_date = now.date_naive();
            // 回看 0..=2 个月:day=31 时最远候选在前前月(2 月无 31 号
            // → from_ymd_opt None → 跳过该月,D7)。
            for months_back in 0i32..=2 {
                let (y, m) = month_shift(now_date, -months_back);
                let Some(date) = NaiveDate::from_ymd_opt(y, m, *day) else {
                    continue;
                };
                if let Some(cand) = local_at(date, hour, minute) {
                    let cand_ms = cand.timestamp_millis();
                    if cand_ms > not_before && cand_ms <= now_ms {
                        return Some(cand_ms);
                    }
                }
            }
            None
        }
    }
}

/// 严格 `> from_ms` 的下一个到期点。**仅** UI 列展示与存库
/// `next_fire_at` —— 触发判定不信任存库值(每 tick 重算,design §2)。
///
/// interval 的展示网格锚定 `from_ms`:调用方传 `due`(本身在网格上)时
/// 展示值 = 真实下一网格点,与 [`most_recent_due`] 互相一致。
pub fn next_fire_display(schedule: &ScheduleSpec, from_ms: i64) -> i64 {
    let from = ms_to_local(from_ms);
    match schedule {
        ScheduleSpec::Interval { every_min } => from_ms + (*every_min).max(1) as i64 * 60_000,
        ScheduleSpec::Daily { at } => {
            if let Ok((hour, minute)) = parse_hh_mm(at) {
                for days_ahead in 0..=1 {
                    let date = (from + Duration::days(days_ahead)).date_naive();
                    if let Some(cand) = local_at(date, hour, minute) {
                        let cand_ms = cand.timestamp_millis();
                        if cand_ms > from_ms {
                            return cand_ms;
                        }
                    }
                }
            }
            from_ms + 86_400_000
        }
        ScheduleSpec::Weekly { weekday, at } => {
            if let Ok((hour, minute)) = parse_hh_mm(at) {
                for days_ahead in 0..=7 {
                    let date = (from + Duration::days(days_ahead)).date_naive();
                    if date.weekday() != *weekday {
                        continue;
                    }
                    if let Some(cand) = local_at(date, hour, minute) {
                        let cand_ms = cand.timestamp_millis();
                        if cand_ms > from_ms {
                            return cand_ms;
                        }
                    }
                }
            }
            from_ms + 7 * 86_400_000
        }
        ScheduleSpec::Hourly { minute } => {
            // 前看 0..=2 个「墙上钟点」(第 2 个是 DST 跳过余量);
            // 解析失败时退化为 +1h(与 daily/weekly 的 fallback 惯例一致)。
            for hours_ahead in 0..=2 {
                let dt = from + Duration::hours(hours_ahead);
                if let Some(cand) = local_at(dt.date_naive(), dt.hour(), *minute) {
                    let cand_ms = cand.timestamp_millis();
                    if cand_ms > from_ms {
                        return cand_ms;
                    }
                }
            }
            from_ms + 3_600_000
        }
        ScheduleSpec::Weekdays { at } => {
            if let Ok((hour, minute)) = parse_hh_mm(at) {
                // 前看 0..=3 天:周五最远的候选是下周一(周末跳过)。
                for days_ahead in 0..=3 {
                    let date = (from + Duration::days(days_ahead)).date_naive();
                    if matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
                        continue;
                    }
                    if let Some(cand) = local_at(date, hour, minute) {
                        let cand_ms = cand.timestamp_millis();
                        if cand_ms > from_ms {
                            return cand_ms;
                        }
                    }
                }
            }
            from_ms + 3 * 86_400_000
        }
        ScheduleSpec::Monthly { day, at } => {
            if let Ok((hour, minute)) = parse_hh_mm(at) {
                let from_date = from.date_naive();
                // 前看 0..=2 个月:day=31 时 1 月末触发后跳过 2 月、
                // 落到 3 月(D7)。
                for months_ahead in 0..=2 {
                    let (y, m) = month_shift(from_date, months_ahead);
                    let Some(date) = NaiveDate::from_ymd_opt(y, m, *day) else {
                        continue;
                    };
                    if let Some(cand) = local_at(date, hour, minute) {
                        let cand_ms = cand.timestamp_millis();
                        if cand_ms > from_ms {
                            return cand_ms;
                        }
                    }
                }
            }
            from_ms + 31 * 86_400_000
        }
    }
}

// ---------------------------------------------------------------------------
// Tests(design §8 纯函数清单)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    /// 固定基准:取「现在」的整分钟截断,避免测试运行跨分钟边界抖动。
    fn base_now_ms() -> i64 {
        let now = Local::now();
        let truncated = now
            .date_naive()
            .and_hms_opt(now.hour(), now.minute(), 0)
            .unwrap();
        Local
            .from_local_datetime(&truncated)
            .earliest()
            .expect("truncated now valid")
            .timestamp_millis()
    }

    fn daily(at: &str) -> ScheduleSpec {
        ScheduleSpec::Daily { at: at.to_string() }
    }

    fn interval(min: u32) -> ScheduleSpec {
        ScheduleSpec::Interval { every_min: min }
    }

    fn weekly(wd: Weekday, at: &str) -> ScheduleSpec {
        ScheduleSpec::Weekly {
            weekday: wd,
            at: at.to_string(),
        }
    }

    fn hourly(minute: u32) -> ScheduleSpec {
        ScheduleSpec::Hourly { minute }
    }

    fn weekdays(at: &str) -> ScheduleSpec {
        ScheduleSpec::Weekdays { at: at.to_string() }
    }

    fn monthly(day: u32, at: &str) -> ScheduleSpec {
        ScheduleSpec::Monthly {
            day,
            at: at.to_string(),
        }
    }

    /// 确定性锚点:本地 `y-m-d h:min` 的 epoch ms。固定历史日期 + 工作日
    /// 时刻 —— 各时区 DST 切换都在周日,工作日的墙上时刻必然存在。
    fn local_ms(y: i32, m: u32, d: u32, h: u32, min: u32) -> i64 {
        let naive = NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, min, 0)
            .unwrap();
        Local
            .from_local_datetime(&naive)
            .earliest()
            .expect("fixed local datetime valid")
            .timestamp_millis()
    }

    // --- 不变量 1:most_recent_due ∈ (not_before, now] ---

    #[test]
    fn most_recent_due_result_strictly_within_window() {
        let now = base_now_ms();
        for nb in [now - 90_000, now - 3_600_000, now - 3 * 86_400_000] {
            for spec in [
                daily("09:00"),
                interval(30),
                weekly(Weekday::Mon, "08:30"),
                hourly(0),
                weekdays("09:00"),
                monthly(1, "09:00"),
            ] {
                if let Some(d) = most_recent_due(&spec, now, nb) {
                    assert!(d > nb, "due {d} must be > not_before {nb}");
                    assert!(d <= now, "due {d} must be <= now {now}");
                }
            }
        }
    }

    // --- 不变量 2:next_fire_display > from ---

    #[test]
    fn next_fire_display_strictly_after_from() {
        let now = base_now_ms();
        for spec in [
            daily("00:00"),
            daily("23:59"),
            interval(1),
            interval(1440),
            hourly(0),
            hourly(59),
            weekdays("09:00"),
            monthly(1, "09:00"),
            monthly(31, "09:00"),
        ] {
            assert!(next_fire_display(&spec, now) > now);
        }
    }

    // --- 不变量 3:两函数互相一致 ---

    #[test]
    fn next_fire_display_is_a_due_point_found_by_most_recent_due() {
        let now = base_now_ms();
        for spec in [
            daily("09:00"),
            interval(30),
            interval(1),
            weekly(Weekday::Mon, "08:30"),
            weekly(Weekday::Sun, "22:00"),
            hourly(30),
            weekdays("09:00"),
            monthly(1, "09:00"),
            monthly(31, "09:00"),
        ] {
            let next = next_fire_display(&spec, now);
            assert!(
                next > now,
                "next_fire_display must be > from, got {next} vs {now}"
            );
            assert_eq!(
                most_recent_due(&spec, next, now),
                Some(next),
                "next_fire_display result must be exactly the most recent due at itself"
            );
        }
    }

    // --- daily 边界 ---

    #[test]
    fn daily_due_today_when_time_already_passed() {
        let now = base_now_ms();
        let now_local = ms_to_local(now);
        let hhmm = format!("{:02}:{:02}", now_local.hour(), now_local.minute());
        // not_before = 昨天同时刻 → 今天的该时刻是最近未消费点。
        let not_before = now - 86_400_000;
        let d = most_recent_due(&daily(&hhmm), now, not_before).expect("today's slot due");
        assert_eq!(d, now, "due equals today's HH:MM == truncated now");
    }

    #[test]
    fn daily_due_yesterday_when_today_slot_not_reached() {
        let now = base_now_ms();
        let now_local = ms_to_local(now);
        // 一个比当前时刻晚至少 1 分钟的 HH:MM(今天未到点)。
        let future_slot = now_local + Duration::minutes(5);
        let hhmm = format!("{:02}:{:02}", future_slot.hour(), future_slot.minute());
        // not_before = 前天 → 昨天的该时刻未被消费,今天未到点 → 命中昨天。
        let not_before = now - 2 * 86_400_000;
        let d = most_recent_due(&daily(&hhmm), now, not_before).expect("yesterday's slot due");
        assert!(d > not_before && d <= now);
        // 该点应是「昨天 HH:MM」:与 now 相差在 [23h, 24h] 量级(含 DST 容差)。
        assert!(
            now - d > 20 * 3_600_000,
            "due must be yesterday's slot, got delta {}ms",
            now - d
        );
    }

    #[test]
    fn daily_returns_none_when_no_unconsumed_slot() {
        let now = base_now_ms();
        let now_local = ms_to_local(now);
        let hhmm = format!("{:02}:{:02}", now_local.hour(), now_local.minute());
        // not_before = 今天已消费该时刻(= now)→ 无窗口内到期点。
        assert_eq!(most_recent_due(&daily(&hhmm), now, now), None);
    }

    // --- weekly 边界 ---

    #[test]
    fn weekly_due_this_week_when_slot_passed() {
        let now = base_now_ms();
        let now_local = ms_to_local(now);
        let hhmm = format!("{:02}:{:02}", now_local.hour(), now_local.minute());
        let wd = now_local.weekday();
        let d = most_recent_due(&weekly(wd, &hhmm), now, now - 3 * 86_400_000)
            .expect("this week's slot due");
        assert_eq!(d, now, "weekly slot today == truncated now");
    }

    #[test]
    fn weekly_skips_other_weekdays() {
        let now = base_now_ms();
        let now_local = ms_to_local(now);
        let hhmm = format!("{:02}:{:02}", now_local.hour(), now_local.minute());
        // 目标 weekday = 昨天 → 最近到期点是昨天的该时刻。
        let yesterday = now_local - Duration::days(1);
        let d = most_recent_due(
            &weekly(yesterday.weekday(), &hhmm),
            now,
            now - 3 * 86_400_000,
        )
        .expect("yesterday slot due");
        let d_local = ms_to_local(d);
        assert_eq!(d_local.date_naive(), yesterday.date_naive());
    }

    // --- interval 锚点与逆推 ---

    #[test]
    fn interval_grid_is_anchored_at_not_before() {
        let anchor = base_now_ms() - 1_234; // 故意不落在整分边界
        let step = 30 * 60_000;
        // now = anchor + 30min + 半步 → 最近网格点 = anchor + 30min。
        let now = anchor + step + step / 2;
        assert_eq!(
            most_recent_due(&interval(30), now, anchor),
            Some(anchor + step)
        );
        // now 恰好落在网格点上 → 该点本身(d <= now)。
        assert_eq!(
            most_recent_due(&interval(30), anchor + step, anchor),
            Some(anchor + step)
        );
        // 不足一步 → None。
        assert_eq!(
            most_recent_due(&interval(30), anchor + step - 1, anchor),
            None
        );
    }

    // --- catch-up 幂等(同一算法,无独立 pass)---

    #[test]
    fn catch_up_is_idempotent_after_accounting_moves_to_due() {
        let now = base_now_ms();
        let spec = daily("09:00");
        // 从未触发、创建于 3 天前:命中最近一个到期点(补一次,不追多次)。
        let due = most_recent_due(&spec, now, now - 3 * 86_400_000).expect("missed slot");
        // 落账 last_fired_at = due 后重评:同一到期点被排除 → None(不双 fire)。
        assert_eq!(most_recent_due(&spec, now, due), None);
        // 下一个到期点到来前持续空转。
        assert_eq!(most_recent_due(&spec, due + 60_000, due), None);
        assert!(most_recent_due(&spec, next_fire_display(&spec, due), due).is_some());
    }

    // --- 不变量 4:interval 无累积漂移(design §8 回归锁)---

    #[test]
    fn interval_has_no_cumulative_drift_with_tick_jitter() {
        // 1min interval 连续模拟 fire:每次评估时刻带 0..45s 的 tick 量化
        // 抖动,落账恒记理论到期点 last_fired_at = due。断言相邻 due 间隔
        // 恒等于网格步长 —— tick 误差不进入下一周期。
        let step_ms = 60_000i64;
        let spec = interval(1);
        let mut not_before = base_now_ms() - 200 * step_ms; // 起点 200 步之前
        let mut prev_due = not_before;
        for i in 0..200 {
            // tick 量化:观测到第 i 个网格点的评估时刻落在 due + jitter
            // (jitter ∈ [0, 45s) 确定性伪随机;30s tick 下最坏一拍慢)。
            let jitter = (i * 7919) % 45_000;
            let now = not_before + step_ms + jitter;
            let due = most_recent_due(&spec, now, not_before)
                .expect("a 1min-interval task evaluated past its next grid point must have a due");
            assert_eq!(due, not_before + step_ms, "due is the exact grid point");
            assert_eq!(
                due - prev_due,
                step_ms,
                "iteration {i}: adjacent due gap must equal the grid step exactly"
            );
            assert!(now - due < step_ms, "tick jitter stays within one step");
            // 落账:账面恒记理论到期点(生产语义,scheduler/mod.rs 同款)。
            not_before = due;
            prev_due = due;
        }
    }

    // --- F2b:hourly 边界(确定性锚点,2026-03-10 是周二)---

    #[test]
    fn hourly_due_this_hour_when_minute_passed() {
        let now = local_ms(2026, 3, 10, 10, 30);
        let nb = local_ms(2026, 3, 10, 8, 0);
        let d = most_recent_due(&hourly(15), now, nb).expect("this hour :15 due");
        assert_eq!(d, local_ms(2026, 3, 10, 10, 15));
    }

    #[test]
    fn hourly_due_previous_hour_when_minute_not_reached() {
        let now = local_ms(2026, 3, 10, 10, 5);
        let nb = local_ms(2026, 3, 10, 8, 0);
        let d = most_recent_due(&hourly(45), now, nb).expect("previous hour :45 due");
        assert_eq!(d, local_ms(2026, 3, 10, 9, 45));
    }

    #[test]
    fn hourly_none_when_slot_consumed() {
        let now = local_ms(2026, 3, 10, 10, 30);
        // not_before 在 09:59 → 10:30 是最近未消费点;消费后同一时刻重评 None。
        let due = most_recent_due(&hourly(30), now, local_ms(2026, 3, 10, 9, 59))
            .expect("10:30 slot due");
        assert_eq!(due, now);
        assert_eq!(most_recent_due(&hourly(30), now, due), None);
    }

    #[test]
    fn hourly_next_fire_skips_to_next_hour_when_minute_passed() {
        let from = local_ms(2026, 3, 10, 10, 30);
        assert_eq!(
            next_fire_display(&hourly(15), from),
            local_ms(2026, 3, 10, 11, 15)
        );
        assert_eq!(
            next_fire_display(&hourly(45), from),
            local_ms(2026, 3, 10, 10, 45)
        );
    }

    // --- F2b:weekdays 边界(2026-03-10 周二 / 03-13 周五 / 03-16 周一)---

    #[test]
    fn weekdays_due_today_on_weekday() {
        let now = local_ms(2026, 3, 10, 12, 0);
        let nb = local_ms(2026, 3, 10, 0, 0);
        let d = most_recent_due(&weekdays("09:00"), now, nb).expect("today's slot due");
        assert_eq!(d, local_ms(2026, 3, 10, 9, 0));
    }

    #[test]
    fn weekdays_monday_falls_back_to_friday() {
        // 周一 07:00:今天 09:00 未到,周六/周日无候选 → 周五 09:00。
        let now = local_ms(2026, 3, 16, 7, 0);
        let nb = local_ms(2026, 3, 10, 0, 0);
        let d = most_recent_due(&weekdays("09:00"), now, nb).expect("friday's slot due");
        assert_eq!(d, local_ms(2026, 3, 13, 9, 0));
        // 周五消费后,周一 09:00 前窗口内无新候选。
        assert_eq!(most_recent_due(&weekdays("09:00"), now, d), None);
    }

    #[test]
    fn weekdays_next_fire_friday_to_monday() {
        let from = local_ms(2026, 3, 13, 10, 0); // 周五 10:00
        assert_eq!(
            next_fire_display(&weekdays("09:00"), from),
            local_ms(2026, 3, 16, 9, 0)
        );
    }

    // --- F2b:monthly 边界(D7:短月跳过)---

    #[test]
    fn monthly_due_this_month_when_slot_passed() {
        let now = local_ms(2026, 3, 20, 12, 0);
        let nb = local_ms(2026, 2, 1, 0, 0);
        let d = most_recent_due(&monthly(15, "09:00"), now, nb).expect("this month 15th due");
        assert_eq!(d, local_ms(2026, 3, 15, 9, 0));
    }

    #[test]
    fn monthly_falls_back_to_previous_month() {
        // 3 月 10 日,15 号未到 → 2 月 15 日。
        let now = local_ms(2026, 3, 10, 12, 0);
        let nb = local_ms(2026, 1, 1, 0, 0);
        let d = most_recent_due(&monthly(15, "09:00"), now, nb).expect("feb 15th due");
        assert_eq!(d, local_ms(2026, 2, 15, 9, 0));
    }

    #[test]
    fn monthly_day31_skips_short_months() {
        // 3 月 1 日:3/31 未来、2/31 不存在(跳过)、1/31 是最近未消费点。
        let now = local_ms(2026, 3, 1, 12, 0);
        let nb = local_ms(2026, 1, 1, 0, 0);
        let d = most_recent_due(&monthly(31, "09:00"), now, nb).expect("jan 31st due");
        assert_eq!(d, local_ms(2026, 1, 31, 9, 0));
        // 1/31 触发后:2 月跳过,下一展示点落在 3/31。
        assert_eq!(
            next_fire_display(&monthly(31, "09:00"), d),
            local_ms(2026, 3, 31, 9, 0)
        );
    }

    // --- parse / validate ---

    #[test]
    fn parse_schedule_accepts_all_three_presets() {
        assert_eq!(
            parse_schedule(r#"{"kind":"daily","at":"09:00"}"#).unwrap(),
            daily("09:00")
        );
        assert_eq!(
            parse_schedule(r#"{"kind":"interval","every_min":30}"#).unwrap(),
            interval(30)
        );
        assert_eq!(
            parse_schedule(r#"{"kind":"weekly","weekday":"mon","at":"09:00"}"#).unwrap(),
            weekly(Weekday::Mon, "09:00")
        );
        // H:MM 单数字小时也接受。
        assert_eq!(
            parse_schedule(r#"{"kind":"daily","at":"9:05"}"#).unwrap(),
            daily("9:05")
        );
    }

    #[test]
    fn parse_schedule_accepts_f2b_presets() {
        assert_eq!(
            parse_schedule(r#"{"kind":"hourly","minute":30}"#).unwrap(),
            hourly(30)
        );
        assert_eq!(
            parse_schedule(r#"{"kind":"weekdays","at":"09:00"}"#).unwrap(),
            weekdays("09:00")
        );
        assert_eq!(
            parse_schedule(r#"{"kind":"monthly","day":15,"at":"09:00"}"#).unwrap(),
            monthly(15, "09:00")
        );
    }

    #[test]
    fn parse_schedule_rejects_malformed_input() {
        assert!(parse_schedule("not json").is_err());
        assert!(parse_schedule(r#"{"kind":"cron","expr":"* * * * *"}"#).is_err());
        assert!(parse_schedule(r#"{"kind":"daily","at":"24:00"}"#).is_err());
        assert!(parse_schedule(r#"{"kind":"daily","at":"09:60"}"#).is_err());
        assert!(parse_schedule(r#"{"kind":"daily","at":"nine"}"#).is_err());
        assert!(parse_schedule(r#"{"kind":"interval","every_min":0}"#).is_err());
        assert!(parse_schedule(r#"{"kind":"weekly","weekday":"funday","at":"09:00"}"#).is_err());
        assert!(parse_schedule(r#"{"kind":"hourly","minute":60}"#).is_err());
        assert!(parse_schedule(r#"{"kind":"monthly","day":0,"at":"09:00"}"#).is_err());
        assert!(parse_schedule(r#"{"kind":"monthly","day":32,"at":"09:00"}"#).is_err());
        assert!(parse_schedule(r#"{"kind":"monthly","day":15,"at":"25:00"}"#).is_err());
        assert!(parse_schedule(r#"{"kind":"weekdays","at":"09:60"}"#).is_err());
    }
}
