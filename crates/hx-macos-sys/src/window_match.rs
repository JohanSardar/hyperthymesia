//! Pure SCK window selection; AX and SCK both use global display point bounds.
use objc2_core_foundation::CGRect;

pub(crate) struct WindowCandidate {
    pub pid: i32,
    pub layer: i32,
    pub on_screen: bool,
    pub bounds: CGRect,
    pub title: Option<String>,
}
fn area(rect: CGRect) -> f64 {
    if [
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    ]
    .iter()
    .all(|v| v.is_finite())
        && rect.size.width > 0.0
        && rect.size.height > 0.0
    {
        rect.size.width * rect.size.height
    } else {
        0.0
    }
}
fn overlap(a: CGRect, b: CGRect) -> f64 {
    let width =
        (a.origin.x + a.size.width).min(b.origin.x + b.size.width) - a.origin.x.max(b.origin.x);
    let height =
        (a.origin.y + a.size.height).min(b.origin.y + b.size.height) - a.origin.y.max(b.origin.y);
    width.max(0.0) * height.max(0.0)
}
/// Never fall back to a different/nonoverlapping window merely because its title
/// matches. Return an index into this exact SCK list, not a pid or AX identifier.
pub(crate) fn select_window(
    windows: &[WindowCandidate],
    pid: i32,
    focused_bounds: CGRect,
    title: Option<&str>,
) -> Option<usize> {
    if area(focused_bounds) == 0.0 {
        return None;
    }
    windows
        .iter()
        .enumerate()
        .filter(|(_, w)| {
            w.pid == pid
                && w.layer == 0
                && w.on_screen
                && area(w.bounds) > 0.0
                && overlap(w.bounds, focused_bounds) > 0.0
        })
        .max_by(|(_, a), (_, b)| {
            overlap(a.bounds, focused_bounds)
                .total_cmp(&overlap(b.bounds, focused_bounds))
                .then_with(|| area(a.bounds).total_cmp(&area(b.bounds)))
                .then_with(|| {
                    (title.is_some() && a.title.as_deref() == title)
                        .cmp(&(title.is_some() && b.title.as_deref() == title))
                })
        })
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_foundation::{CGPoint, CGSize};
    fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
        CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
    }
    fn window(bounds: CGRect, title: &str) -> WindowCandidate {
        WindowCandidate {
            pid: 42,
            layer: 0,
            on_screen: true,
            bounds,
            title: Some(title.into()),
        }
    }
    #[test]
    fn chrome_popups_and_title_mismatch_do_not_hide_main_window() {
        let focus = rect(100., 100., 1200., 800.);
        let mut windows = vec![
            window(rect(100., 100., 100., 40.), "AX title"),
            window(rect(0., 0., 0., 0.), "AX title"),
            window(focus, "Search / Chrome"),
            window(rect(2000., 100., 1400., 900.), "AX title"),
            window(focus, "AX title"),
            window(focus, "AX title"),
            window(focus, "AX title"),
        ];
        windows[4].layer = 1;
        windows[5].on_screen = false;
        windows[6].pid = 43;
        assert_eq!(
            select_window(&windows, 42, focus, Some("AX title")),
            Some(2)
        );
    }
    #[test]
    fn overlap_outranks_title_and_missing_titles_are_allowed() {
        let focus = rect(0., 0., 1200., 800.);
        let mut windows = vec![
            window(rect(0., 0., 600., 800.), "focused"),
            window(focus, ""),
        ];
        windows[1].title = None;
        assert_eq!(select_window(&windows, 42, focus, Some("focused")), Some(1));
        assert_eq!(select_window(&windows, 99, focus, None), None);
        windows[1].bounds.size.width = f64::NAN;
        assert_eq!(select_window(&windows, 42, focus, None), Some(0));
    }
    #[test]
    fn geometry_wins_and_title_breaks_ties() {
        let focus = rect(-1000., 20., 900., 700.);
        let windows = vec![window(focus, "other"), window(focus, "focused")];
        assert_eq!(select_window(&windows, 42, focus, Some("focused")), Some(1));
        assert_eq!(
            select_window(&windows, 42, rect(20., 20., 10., 10.), Some("focused")),
            None
        );
        assert_eq!(
            select_window(&windows, 42, rect(0., 0., 0., 0.), None),
            None
        );
    }
}
