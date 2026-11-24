//! Dates as the user's Mac writes them: `NSDateFormatter`, which is what Settings and every
//! other app use, so a banner timestamp matches the menu-bar clock's conventions.

use objc2::AllocAnyThread;
use objc2_foundation::{NSDate, NSDateFormatter, NSDateFormatterStyle, NSLocale, NSString, NSTimeZone};

use crate::platform::DateStyle;

fn style(s: DateStyle) -> NSDateFormatterStyle {
    match s {
        DateStyle::Short => NSDateFormatterStyle::ShortStyle,
        DateStyle::Medium => NSDateFormatterStyle::MediumStyle,
        DateStyle::Long => NSDateFormatterStyle::LongStyle,
    }
}

pub fn format(millis: i64, date: DateStyle, time: DateStyle, locale: Option<&str>, zone: Option<&str>) -> Option<String> {
    let formatter = NSDateFormatter::new();
    formatter.setDateStyle(style(date));
    formatter.setTimeStyle(style(time));
    if let Some(id) = locale {
        let locale = NSLocale::initWithLocaleIdentifier(NSLocale::alloc(), &NSString::from_str(id));
        formatter.setLocale(Some(&locale));
    }
    if let Some(name) = zone {
        let tz = NSTimeZone::timeZoneWithName(&NSString::from_str(name))?;
        formatter.setTimeZone(Some(&tz));
    }
    let at = NSDate::dateWithTimeIntervalSince1970(millis as f64 / 1000.0);
    Some(formatter.stringFromDate(&at).to_string())
}
