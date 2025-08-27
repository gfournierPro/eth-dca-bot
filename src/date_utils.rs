use chrono::{Datelike, Duration, NaiveDate, Utc, Weekday};

/// Check if the given date is the last Monday of its month
pub fn is_last_monday_of_month(date: NaiveDate) -> bool {
    // Check if it's a Monday
    if date.weekday() != Weekday::Mon {
        return false;
    }

    // Find the last Monday of the month
    let last_monday = get_last_monday_of_month(date.year(), date.month());
    
    date == last_monday
}

/// Check if today is the last Monday of the current month
pub fn is_today_last_monday_of_month() -> bool {
    let today = Utc::now().date_naive();
    is_last_monday_of_month(today)
}

/// Check if we're in the last week before a new month starts
/// This returns true if today is within 7 days of the month end and it's Monday or later in the week
pub fn is_last_week_before_new_month() -> bool {
    let today = Utc::now().date_naive();
    let last_day_of_month = get_last_day_of_month(today.year(), today.month());
    let days_until_month_end = (last_day_of_month - today).num_days();
    
    // Check if we're within 7 days of month end
    if days_until_month_end <= 7 && days_until_month_end >= 0 {
        // Check if we've already passed the last Monday or if today is the last Monday
        let last_monday = get_last_monday_of_month(today.year(), today.month());
        return today >= last_monday;
    }
    
    false
}

/// Get the last Monday of a given month
fn get_last_monday_of_month(year: i32, month: u32) -> NaiveDate {
    let last_day = get_last_day_of_month(year, month);
    
    // Find the last Monday by going backwards from the last day
    let mut current_date = last_day;
    while current_date.weekday() != Weekday::Mon {
        current_date = current_date - Duration::days(1);
    }
    
    current_date
}

/// Get the last day of a given month
fn get_last_day_of_month(year: i32, month: u32) -> NaiveDate {
    // Get the first day of the next month, then subtract one day
    let next_month = if month == 12 { 1 } else { month + 1 };
    let next_year = if month == 12 { year + 1 } else { year };
    
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .unwrap()
        .pred_opt()
        .unwrap()
}

/// Check if we should perform a withdrawal check
/// This returns true if:
/// 1. Today is the last Monday of the month, OR
/// 2. We're in the last week before a new month and the last Monday has already passed
pub fn should_check_withdrawal() -> bool {
    is_today_last_monday_of_month() || is_last_week_before_new_month()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn test_is_last_monday_of_month() {
        // Test case: January 2024 - last Monday is January 29, 2024
        let jan_29_2024 = NaiveDate::from_ymd_opt(2024, 1, 29).unwrap();
        assert!(is_last_monday_of_month(jan_29_2024));
        
        // Test case: January 22, 2024 is a Monday but not the last Monday
        let jan_22_2024 = NaiveDate::from_ymd_opt(2024, 1, 22).unwrap();
        assert!(!is_last_monday_of_month(jan_22_2024));
        
        // Test case: January 30, 2024 is not a Monday
        let jan_30_2024 = NaiveDate::from_ymd_opt(2024, 1, 30).unwrap();
        assert!(!is_last_monday_of_month(jan_30_2024));
    }

    #[test]
    fn test_get_last_monday_of_month() {
        // Test January 2024 - last Monday should be January 29
        let last_monday_jan_2024 = get_last_monday_of_month(2024, 1);
        assert_eq!(last_monday_jan_2024, NaiveDate::from_ymd_opt(2024, 1, 29).unwrap());
        
        // Test February 2024 - last Monday should be February 26
        let last_monday_feb_2024 = get_last_monday_of_month(2024, 2);
        assert_eq!(last_monday_feb_2024, NaiveDate::from_ymd_opt(2024, 2, 26).unwrap());
    }
}
