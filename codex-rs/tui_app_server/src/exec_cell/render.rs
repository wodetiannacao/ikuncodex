use std::time::Duration;
use std::time::Instant;

use super::exec_output_details_expanded;
use super::model::CommandOutput;
use super::model::ExecCall;
use super::model::ExecCell;
use crate::exec_command::strip_bash_lc_and_escape;
use crate::history_cell::HistoryCell;
use crate::line_truncation::truncate_line_with_ellipsis_if_overflow;
use crate::render::highlight::highlight_bash_to_lines;
use crate::render::line_utils::prefix_lines;
use crate::render::line_utils::push_owned_lines;
use crate::shimmer::shimmer_spans;
use crate::wrapping::RtOptions;
use crate::wrapping::adaptive_wrap_line;
use codex_ansi_escape::ansi_escape_line;
use codex_protocol::parse_command::ParsedCommand;
use codex_protocol::protocol::ExecCommandSource;
use codex_shell_command::bash::extract_bash_command;
use codex_utils_elapsed::format_duration;
use itertools::Itertools;
use ratatui::prelude::*;
use ratatui::style::Modifier;
use ratatui::style::Stylize;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use unicode_width::UnicodeWidthStr;

pub(crate) const TOOL_CALL_MAX_LINES: usize = 5;
const MAX_INTERACTION_PREVIEW_CHARS: usize = 80;

pub(crate) struct OutputLinesParams {
    pub(crate) line_limit: usize,
    pub(crate) only_err: bool,
    pub(crate) include_angle_pipe: bool,
    pub(crate) include_prefix: bool,
}

pub(crate) fn new_active_exec_command(
    call_id: String,
    command: Vec<String>,
    parsed: Vec<ParsedCommand>,
    source: ExecCommandSource,
    interaction_input: Option<String>,
    animations_enabled: bool,
) -> ExecCell {
    ExecCell::new(
        ExecCall {
            call_id,
            command,
            parsed,
            output: None,
            source,
            start_time: Some(Instant::now()),
            duration: None,
            interaction_input,
        },
        animations_enabled,
    )
}

fn format_unified_exec_interaction(command: &[String], input: Option<&str>) -> String {
    let command_display = if let Some((_, script)) = extract_bash_command(command) {
        script.to_string()
    } else {
        command.join(" ")
    };
    match input {
        Some(data) if !data.is_empty() => {
            let preview = summarize_interaction_input(data);
            format!("Interacted with `{command_display}`, sent `{preview}`")
        }
        _ => format!("Waited for `{command_display}`"),
    }
}

fn summarize_interaction_input(input: &str) -> String {
    let single_line = input.replace('\n', "\\n");
    let sanitized = single_line.replace('`', "\\`");
    if sanitized.chars().count() <= MAX_INTERACTION_PREVIEW_CHARS {
        return sanitized;
    }

    let mut preview = String::new();
    for ch in sanitized.chars().take(MAX_INTERACTION_PREVIEW_CHARS) {
        preview.push(ch);
    }
    preview.push_str("...");
    preview
}

#[derive(Clone)]
pub(crate) struct OutputLines {
    pub(crate) lines: Vec<Line<'static>>,
    #[allow(dead_code)]
    pub(crate) omitted: Option<usize>,
}

pub(crate) fn output_lines(
    output: Option<&CommandOutput>,
    params: OutputLinesParams,
) -> OutputLines {
    let OutputLinesParams {
        line_limit,
        only_err,
        include_angle_pipe,
        include_prefix,
    } = params;
    let CommandOutput {
        aggregated_output, ..
    } = match output {
        Some(output) if only_err && output.exit_code == 0 => {
            return OutputLines {
                lines: Vec::new(),
                omitted: None,
            };
        }
        Some(output) => output,
        None => {
            return OutputLines {
                lines: Vec::new(),
                omitted: None,
            };
        }
    };

    let src = aggregated_output;
    let lines: Vec<&str> = src.lines().collect();
    let total = lines.len();
    let mut out: Vec<Line<'static>> = Vec::new();

    let head_end = total.min(line_limit);
    for (i, raw) in lines[..head_end].iter().enumerate() {
        let mut line = ansi_escape_line(raw);
        let prefix = if !include_prefix {
            ""
        } else if i == 0 && include_angle_pipe {
            "  └ "
        } else {
            "    "
        };
        line.spans.insert(0, prefix.into());
        line.spans.iter_mut().for_each(|span| {
            span.style = span.style.add_modifier(Modifier::DIM);
        });
        out.push(line);
    }

    let show_ellipsis = total > 2 * line_limit;
    let omitted = if show_ellipsis {
        Some(total - 2 * line_limit)
    } else {
        None
    };
    if show_ellipsis {
        let omitted = total - 2 * line_limit;
        out.push(format!("… +{omitted} lines").into());
    }

    let tail_start = if show_ellipsis {
        total - line_limit
    } else {
        head_end
    };
    for raw in lines[tail_start..].iter() {
        let mut line = ansi_escape_line(raw);
        if include_prefix {
            line.spans.insert(0, "    ".into());
        }
        line.spans.iter_mut().for_each(|span| {
            span.style = span.style.add_modifier(Modifier::DIM);
        });
        out.push(line);
    }

    OutputLines {
        lines: out,
        omitted,
    }
}

pub(crate) fn spinner(start_time: Option<Instant>, animations_enabled: bool) -> Span<'static> {
    if !animations_enabled {
        return "•".dim();
    }
    let elapsed = start_time.map(|st| st.elapsed()).unwrap_or_default();
    if supports_color::on_cached(supports_color::Stream::Stdout)
        .map(|level| level.has_16m)
        .unwrap_or(false)
    {
        shimmer_spans("•")[0].clone()
    } else {
        let blink_on = (elapsed.as_millis() / 600).is_multiple_of(2);
        if blink_on { "•".into() } else { "◦".dim() }
    }
}

impl HistoryCell for ExecCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        if self.is_exploring_cell() {
            self.exploring_display_lines(width)
        } else {
            self.transcript_lines(width)
        }
    }

    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        if !exec_output_details_expanded() {
            return vec![self.aggregate_command_summary_line(width)];
        }

        self.iter_calls()
            .map(|call| {
                let (bullet, title) = self.command_status_parts(call);
                let cmd_display = if call.is_unified_exec_interaction() {
                    format_unified_exec_interaction(
                        &call.command,
                        call.interaction_input.as_deref(),
                    )
                } else {
                    strip_bash_lc_and_escape(&call.command)
                };
                let highlighted_lines = highlight_bash_to_lines(&cmd_display);
                Self::compact_command_line(call, width, bullet, title, &highlighted_lines)
            })
            .collect()
    }
}

impl ExecCell {
    fn command_status_parts(&self, call: &ExecCall) -> (Span<'static>, &'static str) {
        let success = call.output.as_ref().map(|o| o.exit_code == 0);
        let bullet = match success {
            Some(true) => "•".green().bold(),
            Some(false) => "•".red().bold(),
            None => spinner(call.start_time, self.animations_enabled()),
        };
        let title = if call.is_unified_exec_interaction() {
            ""
        } else if self.is_active() {
            "Running"
        } else if call.is_user_shell_command() {
            "You ran"
        } else {
            "Ran"
        };
        (bullet, title)
    }

    fn exploring_display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut out: Vec<Line<'static>> = Vec::new();
        out.push(Line::from(vec![
            if self.is_active() {
                spinner(self.active_start_time(), self.animations_enabled())
            } else {
                "•".dim()
            },
            " ".into(),
            if self.is_active() {
                "Exploring".bold()
            } else {
                "Explored".bold()
            },
        ]));

        let mut calls = self.calls.clone();
        let mut out_indented = Vec::new();
        while !calls.is_empty() {
            let mut call = calls.remove(0);
            if call
                .parsed
                .iter()
                .all(|parsed| matches!(parsed, ParsedCommand::Read { .. }))
            {
                while let Some(next) = calls.first() {
                    if next
                        .parsed
                        .iter()
                        .all(|parsed| matches!(parsed, ParsedCommand::Read { .. }))
                    {
                        call.parsed.extend(next.parsed.clone());
                        calls.remove(0);
                    } else {
                        break;
                    }
                }
            }

            let reads_only = call
                .parsed
                .iter()
                .all(|parsed| matches!(parsed, ParsedCommand::Read { .. }));

            let call_lines: Vec<(&str, Vec<Span<'static>>)> = if reads_only {
                let names = call
                    .parsed
                    .iter()
                    .map(|parsed| match parsed {
                        ParsedCommand::Read { name, .. } => name.clone(),
                        _ => unreachable!(),
                    })
                    .unique();
                vec![(
                    "Read",
                    Itertools::intersperse(names.into_iter().map(Into::into), ", ".dim()).collect(),
                )]
            } else {
                let mut lines = Vec::new();
                for parsed in &call.parsed {
                    match parsed {
                        ParsedCommand::Read { name, .. } => {
                            lines.push(("Read", vec![name.clone().into()]));
                        }
                        ParsedCommand::ListFiles { cmd, path } => {
                            lines.push(("List", vec![path.clone().unwrap_or(cmd.clone()).into()]));
                        }
                        ParsedCommand::Search { cmd, query, path } => {
                            let spans = match (query, path) {
                                (Some(q), Some(p)) => {
                                    vec![q.clone().into(), " in ".dim(), p.clone().into()]
                                }
                                (Some(q), None) => vec![q.clone().into()],
                                _ => vec![cmd.clone().into()],
                            };
                            lines.push(("Search", spans));
                        }
                        ParsedCommand::Unknown { cmd } => {
                            lines.push(("Run", vec![cmd.clone().into()]));
                        }
                    }
                }
                lines
            };

            for (title, line) in call_lines {
                let line = Line::from(line);
                let initial_indent = Line::from(vec![title.cyan(), " ".into()]);
                let subsequent_indent = " ".repeat(initial_indent.width()).into();
                let wrapped = adaptive_wrap_line(
                    &line,
                    RtOptions::new(width as usize)
                        .initial_indent(initial_indent)
                        .subsequent_indent(subsequent_indent),
                );
                push_owned_lines(&wrapped, &mut out_indented);
            }
        }

        out.extend(prefix_lines(out_indented, "  └ ".dim(), "    ".into()));
        out
    }

    fn aggregate_command_summary_line(&self, width: u16) -> Line<'static> {
        #[derive(Default)]
        struct AggregateSummary {
            running: usize,
            ran: usize,
            user_ran: usize,
            failed: usize,
            interaction: usize,
            total_duration: Duration,
            command_previews: Vec<String>,
        }

        let summary = self
            .iter_calls()
            .fold(AggregateSummary::default(), |mut summary, call| {
                if call.is_unified_exec_interaction() {
                    summary.interaction += 1;
                } else if call.output.is_none() {
                    summary.running += 1;
                } else if call.is_user_shell_command() {
                    summary.user_ran += 1;
                } else if call
                    .output
                    .as_ref()
                    .is_some_and(|output| output.exit_code != 0)
                {
                    summary.failed += 1;
                } else {
                    summary.ran += 1;
                }

                if let Some(duration) = call
                    .duration
                    .or_else(|| call.start_time.map(|start_time| start_time.elapsed()))
                {
                    summary.total_duration += duration;
                }

                if summary.command_previews.len() < 3 {
                    let preview = Self::command_preview(call);
                    if !preview.is_empty()
                        && !summary.command_previews.iter().any(|item| item == &preview)
                    {
                        summary.command_previews.push(preview);
                    }
                }

                summary
            });

        let bullet = if summary.running > 0 {
            spinner(self.active_start_time(), self.animations_enabled())
        } else if summary.failed > 0 {
            "•".red().bold()
        } else {
            "•".green().bold()
        };

        let mut parts = Vec::new();
        if summary.running > 0 {
            parts.push(format!("Running {}", summary.running));
        }
        if summary.ran > 0 {
            parts.push(format!("Ran {}", summary.ran));
        }
        if summary.user_ran > 0 {
            parts.push(format!("You ran {}", summary.user_ran));
        }
        if summary.failed > 0 {
            parts.push(format!("Failed {}", summary.failed));
        }
        if summary.interaction > 0 {
            parts.push(format!("Input {}", summary.interaction));
        }
        if !self.calls.is_empty() {
            parts.push(format!("{} commands", self.calls.len()));
        }
        if !summary.command_previews.is_empty() {
            let hidden = self
                .calls
                .len()
                .saturating_sub(summary.command_previews.len());
            let preview_text = if hidden > 0 {
                format!("{} +{} more", summary.command_previews.join(", "), hidden)
            } else {
                summary.command_previews.join(", ")
            };
            parts.push(preview_text);
        }
        if summary.total_duration > Duration::ZERO {
            parts.push(format!("{} total", format_duration(summary.total_duration)));
        }

        let mut line = Line::from(vec![bullet, " ".into()]);
        let summary_text = if parts.is_empty() {
            "No command activity".to_string()
        } else {
            parts.join(" · ")
        };
        line.push_span(summary_text);

        truncate_line_with_ellipsis_if_overflow(line, width as usize)
    }

    fn command_preview(call: &ExecCall) -> String {
        let preview = if call.is_unified_exec_interaction() {
            format_unified_exec_interaction(&call.command, call.interaction_input.as_deref())
        } else {
            strip_bash_lc_and_escape(&call.command)
        };
        preview
            .split_whitespace()
            .take(8)
            .join(" ")
            .chars()
            .take(48)
            .collect::<String>()
    }

    fn compact_command_line(
        call: &ExecCall,
        width: u16,
        bullet: Span<'static>,
        title: &str,
        highlighted_lines: &[Line<'static>],
    ) -> Line<'static> {
        let mut line = if call.is_unified_exec_interaction() {
            Line::from(vec![bullet, " ".into()])
        } else {
            Line::from(vec![
                bullet,
                " ".into(),
                Span::styled(
                    title.to_owned(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                " ".into(),
            ])
        };

        if let Some(first_line) = highlighted_lines.first() {
            line.extend(first_line.spans.clone());
            if highlighted_lines.len() > 1 {
                line.push_span(" …".dim());
            }
        }

        if let Some(output) = call.output.as_ref() {
            if let Some(duration) = call.duration {
                line.push_span(Span::raw(" "));
                line.push_span(format!("({})", format_duration(duration)).dim());
            }
            if output.exit_code != 0 {
                line.push_span(Span::raw(" "));
                line.push_span(format!("[exit {}]", output.exit_code).red().dim());
            }
        }

        truncate_line_with_ellipsis_if_overflow(line, width as usize)
    }

    #[allow(dead_code)]
    fn limit_lines_from_start(lines: &[Line<'static>], keep: usize) -> Vec<Line<'static>> {
        if lines.len() <= keep {
            return lines.to_vec();
        }
        if keep == 0 {
            return vec![Self::ellipsis_line(lines.len())];
        }

        let mut out: Vec<Line<'static>> = lines[..keep].to_vec();
        out.push(Self::ellipsis_line(lines.len() - keep));
        out
    }

    /// Truncates a list of lines to fit within `max_rows` viewport rows,
    /// keeping a head portion and a tail portion with an ellipsis line
    /// in between.
    ///
    /// `max_rows` is measured in viewport rows (the actual space a line
    /// occupies after `Paragraph::wrap`), not logical lines. Each line's
    /// row cost is computed via `Paragraph::line_count` at the given
    /// `width`. This ensures that a single logical line containing a
    /// long URL (which wraps to several viewport rows) is properly
    /// accounted for.
    ///
    /// The ellipsis message reports the number of omitted *lines*
    /// (logical, not rows) to keep the count stable across terminal
    /// widths. `omitted_hint` carries forward any previously reported
    /// omitted count (from upstream truncation); `ellipsis_prefix`
    /// prepends the output gutter prefix to the ellipsis line.
    #[allow(dead_code)]
    fn truncate_lines_middle(
        lines: &[Line<'static>],
        max_rows: usize,
        width: u16,
        omitted_hint: Option<usize>,
        ellipsis_prefix: Option<Line<'static>>,
    ) -> Vec<Line<'static>> {
        let width = width.max(1);
        if max_rows == 0 {
            return Vec::new();
        }
        let line_rows: Vec<usize> = lines
            .iter()
            .map(|line| {
                let is_whitespace_only = line
                    .spans
                    .iter()
                    .all(|span| span.content.chars().all(char::is_whitespace));
                if is_whitespace_only {
                    line.width().div_ceil(usize::from(width)).max(1)
                } else {
                    Paragraph::new(Text::from(vec![line.clone()]))
                        .wrap(Wrap { trim: false })
                        .line_count(width)
                        .max(1)
                }
            })
            .collect();
        let total_rows: usize = line_rows.iter().sum();
        if total_rows <= max_rows {
            return lines.to_vec();
        }
        if max_rows == 1 {
            // Carry forward any previously omitted count and add any
            // additionally hidden content lines from this truncation.
            let base = omitted_hint.unwrap_or(0);
            // When an existing ellipsis is present, `lines` already includes
            // that single representation line; exclude it from the count of
            // additionally omitted content lines.
            let extra = lines
                .len()
                .saturating_sub(usize::from(omitted_hint.is_some()));
            let omitted = base + extra;
            return vec![Self::ellipsis_line_with_prefix(
                omitted,
                ellipsis_prefix.as_ref(),
            )];
        }

        let head_budget = (max_rows - 1) / 2;
        let tail_budget = max_rows - head_budget - 1;
        let mut head_lines: Vec<Line<'static>> = Vec::new();
        let mut head_rows = 0usize;
        let mut head_end = 0usize;
        while head_end < lines.len() {
            let line_row_count = line_rows[head_end];
            if head_rows + line_row_count > head_budget {
                break;
            }
            head_rows += line_row_count;
            head_lines.push(lines[head_end].clone());
            head_end += 1;
        }

        let mut tail_lines_reversed: Vec<Line<'static>> = Vec::new();
        let mut tail_rows = 0usize;
        let mut tail_start = lines.len();
        while tail_start > head_end {
            let idx = tail_start - 1;
            let line_row_count = line_rows[idx];
            if tail_rows + line_row_count > tail_budget {
                break;
            }
            tail_rows += line_row_count;
            tail_lines_reversed.push(lines[idx].clone());
            tail_start -= 1;
        }

        let mut out = head_lines;
        let base = omitted_hint.unwrap_or(0);
        let additional = lines
            .len()
            .saturating_sub(out.len() + tail_lines_reversed.len())
            .saturating_sub(usize::from(omitted_hint.is_some()));
        out.push(Self::ellipsis_line_with_prefix(
            base + additional,
            ellipsis_prefix.as_ref(),
        ));

        out.extend(tail_lines_reversed.into_iter().rev());

        out
    }

    #[allow(dead_code)]
    fn ellipsis_line(omitted: usize) -> Line<'static> {
        Line::from(vec![format!("… +{omitted} lines").dim()])
    }

    /// Builds an ellipsis line (`… +N lines`) with an optional leading
    /// prefix so the ellipsis aligns with the output gutter.
    #[allow(dead_code)]
    fn ellipsis_line_with_prefix(omitted: usize, prefix: Option<&Line<'static>>) -> Line<'static> {
        let mut line = prefix.cloned().unwrap_or_default();
        line.push_span(format!("… +{omitted} lines").dim());
        line
    }
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
struct PrefixedBlock {
    initial_prefix: &'static str,
    subsequent_prefix: &'static str,
}

#[allow(dead_code)]
impl PrefixedBlock {
    const fn new(initial_prefix: &'static str, subsequent_prefix: &'static str) -> Self {
        Self {
            initial_prefix,
            subsequent_prefix,
        }
    }

    fn wrap_width(self, total_width: u16) -> usize {
        let prefix_width = UnicodeWidthStr::width(self.initial_prefix)
            .max(UnicodeWidthStr::width(self.subsequent_prefix));
        usize::from(total_width).saturating_sub(prefix_width).max(1)
    }
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
struct ExecDisplayLayout {
    command_continuation: PrefixedBlock,
    command_continuation_max_lines: usize,
    output_block: PrefixedBlock,
    output_max_lines: usize,
}

#[allow(dead_code)]
impl ExecDisplayLayout {
    const fn new(
        command_continuation: PrefixedBlock,
        command_continuation_max_lines: usize,
        output_block: PrefixedBlock,
        output_max_lines: usize,
    ) -> Self {
        Self {
            command_continuation,
            command_continuation_max_lines,
            output_block,
            output_max_lines,
        }
    }
}

#[allow(dead_code)]
const EXEC_DISPLAY_LAYOUT: ExecDisplayLayout = ExecDisplayLayout::new(
    PrefixedBlock::new("  │ ", "  │ "),
    /*command_continuation_max_lines*/ 2,
    PrefixedBlock::new("  └ ", "    "),
    /*output_max_lines*/ 5,
);

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::protocol::ExecCommandSource;
    use pretty_assertions::assert_eq;

    #[test]
    fn truncate_lines_middle_keeps_omitted_count_in_line_units() {
        let lines = vec![
            Line::from("  └ short"),
            Line::from("    this-is-a-very-long-token-that-wraps-many-rows"),
            Line::from("    … +4 lines"),
            Line::from("    tail"),
        ];

        let truncated =
            ExecCell::truncate_lines_middle(&lines, 2, 12, Some(4), Some(Line::from("    ".dim())));
        let rendered: Vec<String> = truncated
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();

        assert!(
            rendered.iter().any(|line| line.contains("… +6 lines")),
            "expected omitted hint to count hidden lines (not wrapped rows), got: {rendered:?}"
        );
    }

    #[test]
    fn truncate_lines_middle_does_not_truncate_blank_prefixed_output_lines() {
        let mut lines = vec![Line::from("  └ start")];
        lines.extend(std::iter::repeat_n(Line::from("    "), 26));
        lines.push(Line::from("    end"));

        let truncated = ExecCell::truncate_lines_middle(&lines, 28, 80, None, None);

        assert_eq!(truncated, lines);
    }

    #[test]
    fn command_display_does_not_split_long_url_token() {
        let previous = crate::exec_cell::set_exec_output_details_expanded(true);
        let url = "http://example.com/long-url-with-dashes-wider-than-terminal-window/blah-blah-blah-text/more-gibberish-text";

        let call = ExecCall {
            call_id: "call-id".to_string(),
            command: vec!["bash".into(), "-lc".into(), format!("echo {url}")],
            parsed: Vec::new(),
            output: None,
            source: ExecCommandSource::UserShell,
            start_time: None,
            duration: None,
            interaction_input: None,
        };

        let cell = ExecCell::new(call, false);
        let rendered: Vec<String> = cell
            .display_lines(36)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();

        assert_eq!(
            rendered.len(),
            1,
            "expanded command view should stay single-line"
        );
        assert!(
            rendered[0].contains("echo http://example.com"),
            "expected command preview to preserve the command prefix, got: {rendered:?}"
        );
        assert!(
            rendered[0].contains('…'),
            "expected long command preview to truncate with ellipsis, got: {rendered:?}"
        );

        let _ = crate::exec_cell::set_exec_output_details_expanded(previous);
    }

    #[test]
    fn exploring_display_does_not_split_long_url_like_search_query() {
        let url_like = "example.test/api/v1/projects/alpha-team/releases/2026-02-17/builds/1234567890/artifacts/reports/performance/summary/detail/with/a/very/long/path";
        let call = ExecCall {
            call_id: "call-id".to_string(),
            command: vec!["bash".into(), "-lc".into(), "rg foo".into()],
            parsed: vec![ParsedCommand::Search {
                cmd: format!("rg {url_like}"),
                query: Some(url_like.to_string()),
                path: None,
            }],
            output: None,
            source: ExecCommandSource::Agent,
            start_time: None,
            duration: None,
            interaction_input: None,
        };

        let cell = ExecCell::new(call, false);
        let rendered: Vec<String> = cell
            .display_lines(36)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();

        assert_eq!(
            rendered
                .iter()
                .filter(|line| line.contains(url_like))
                .count(),
            1,
            "expected full URL-like query in one rendered line, got: {rendered:?}"
        );
    }

    #[test]
    fn output_display_does_not_split_long_url_like_token_without_scheme() {
        let previous = crate::exec_cell::set_exec_output_details_expanded(true);
        let url = "example.test/api/v1/projects/alpha-team/releases/2026-02-17/builds/1234567890/artifacts/reports/performance/summary/detail/session_id=abc123def456ghi789jkl012mno345pqr678";

        let call = ExecCall {
            call_id: "call-id".to_string(),
            command: vec!["bash".into(), "-lc".into(), url.to_string()],
            parsed: Vec::new(),
            output: Some(CommandOutput {
                exit_code: 0,
                formatted_output: String::new(),
                aggregated_output: String::new(),
            }),
            source: ExecCommandSource::UserShell,
            start_time: None,
            duration: None,
            interaction_input: None,
        };

        let cell = ExecCell::new(call, false);
        let rendered: Vec<String> = cell
            .display_lines(36)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();

        assert_eq!(
            rendered.len(),
            1,
            "expanded command view should stay single-line"
        );
        assert!(
            rendered[0].contains("example.test/api/v1"),
            "expected URL-like command prefix to remain visible, got: {rendered:?}"
        );
        assert!(
            rendered[0].contains('…'),
            "expected long URL-like command preview to truncate with ellipsis, got: {rendered:?}"
        );

        let _ = crate::exec_cell::set_exec_output_details_expanded(previous);
    }

    #[test]
    fn desired_transcript_height_accounts_for_wrapped_url_like_rows() {
        let previous = crate::exec_cell::set_exec_output_details_expanded(true);
        let url = "https://example.test/api/v1/projects/alpha-team/releases/2026-02-17/builds/1234567890/artifacts/reports/performance/summary/detail/with/a/very/long/path/that/keeps/going/for/testing/purposes";
        let call = ExecCall {
            call_id: "call-id".to_string(),
            command: vec!["bash".into(), "-lc".into(), "echo done".into()],
            parsed: Vec::new(),
            output: Some(CommandOutput {
                exit_code: 0,
                formatted_output: url.to_string(),
                aggregated_output: url.to_string(),
            }),
            source: ExecCommandSource::Agent,
            start_time: None,
            duration: None,
            interaction_input: None,
        };

        let cell = ExecCell::new(call, false);
        let width: u16 = 36;
        let logical_height = cell.transcript_lines(width).len() as u16;
        let wrapped_height = cell.desired_transcript_height(width);

        assert_eq!(
            wrapped_height, logical_height,
            "compact expanded command rows should remain one viewport row even when raw output contains a long URL, logical_height={logical_height}, wrapped_height={wrapped_height}"
        );

        let _ = crate::exec_cell::set_exec_output_details_expanded(previous);
    }

    #[test]
    fn command_display_defaults_to_single_compact_line() {
        let previous = crate::exec_cell::set_exec_output_details_expanded(false);

        let call = ExecCall {
            call_id: "call-id".to_string(),
            command: vec!["bash".into(), "-lc".into(), "echo hello".into()],
            parsed: Vec::new(),
            output: Some(CommandOutput {
                exit_code: 0,
                formatted_output: "hello\nworld".to_string(),
                aggregated_output: "hello\nworld".to_string(),
            }),
            source: ExecCommandSource::Agent,
            start_time: None,
            duration: Some(std::time::Duration::from_secs(1)),
            interaction_input: None,
        };

        let cell = ExecCell::new(call, false);
        let rendered = cell.display_lines(80);
        assert_eq!(
            rendered.len(),
            1,
            "collapsed mode should render one aggregate line"
        );

        let blob = rendered
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(
            blob.contains("Ran 1"),
            "expected collapsed summary counts: {blob:?}"
        );
        assert!(
            blob.contains("echo hello"),
            "expected collapsed summary to keep a representative command preview: {blob:?}"
        );
        assert!(
            !blob.contains("world"),
            "collapsed mode should hide detailed output: {blob:?}"
        );

        crate::exec_cell::set_exec_output_details_expanded(previous);
    }

    #[test]
    fn transcript_lines_default_to_aggregate_command_summary() {
        let previous = crate::exec_cell::set_exec_output_details_expanded(false);

        let mut cell = ExecCell::new(
            ExecCall {
                call_id: "call-id".to_string(),
                command: vec!["bash".into(), "-lc".into(), "echo hello".into()],
                parsed: Vec::new(),
                output: Some(CommandOutput {
                    exit_code: 0,
                    formatted_output: "hello\nworld".to_string(),
                    aggregated_output: "hello\nworld".to_string(),
                }),
                source: ExecCommandSource::Agent,
                start_time: None,
                duration: Some(std::time::Duration::from_secs(1)),
                interaction_input: None,
            },
            false,
        );
        cell.calls.push(ExecCall {
            call_id: "call-id-2".to_string(),
            command: vec!["bash".into(), "-lc".into(), "cargo test".into()],
            parsed: Vec::new(),
            output: Some(CommandOutput {
                exit_code: 0,
                formatted_output: "ok".to_string(),
                aggregated_output: "ok".to_string(),
            }),
            source: ExecCommandSource::Agent,
            start_time: None,
            duration: Some(std::time::Duration::from_secs(2)),
            interaction_input: None,
        });
        let rendered = cell.transcript_lines(80);
        assert_eq!(
            rendered.len(),
            1,
            "collapsed transcript should render one aggregate line"
        );

        let blob = rendered
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(
            blob.contains("Ran 2"),
            "expected aggregate transcript summary: {blob:?}"
        );
        assert!(
            blob.contains("echo hello"),
            "expected aggregate transcript summary to keep preview text: {blob:?}"
        );
        assert!(
            blob.contains("cargo test"),
            "expected aggregate transcript summary to keep multiple preview texts: {blob:?}"
        );
        assert!(
            blob.contains("3.00s total"),
            "expected total duration in aggregate transcript summary: {blob:?}"
        );

        crate::exec_cell::set_exec_output_details_expanded(previous);
    }

    #[test]
    fn display_lines_expand_to_one_command_per_line() {
        let previous = crate::exec_cell::set_exec_output_details_expanded(true);

        let mut cell = ExecCell::new(
            ExecCall {
                call_id: "call-1".to_string(),
                command: vec!["bash".into(), "-lc".into(), "echo hello".into()],
                parsed: Vec::new(),
                output: Some(CommandOutput {
                    exit_code: 0,
                    formatted_output: "hello".to_string(),
                    aggregated_output: "hello".to_string(),
                }),
                source: ExecCommandSource::Agent,
                start_time: None,
                duration: Some(std::time::Duration::from_secs(1)),
                interaction_input: None,
            },
            false,
        );
        cell.calls.push(ExecCall {
            call_id: "call-2".to_string(),
            command: vec!["bash".into(), "-lc".into(), "cargo test".into()],
            parsed: Vec::new(),
            output: Some(CommandOutput {
                exit_code: 0,
                formatted_output: "ok".to_string(),
                aggregated_output: "ok".to_string(),
            }),
            source: ExecCommandSource::Agent,
            start_time: None,
            duration: Some(std::time::Duration::from_secs(2)),
            interaction_input: None,
        });

        let rendered = cell.display_lines(80);
        assert_eq!(
            rendered.len(),
            2,
            "expanded display should show one line per command"
        );

        let rendered_lines = rendered
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(
            rendered_lines
                .iter()
                .any(|line| line.contains("Ran echo hello")),
            "expected expanded display to include first command: {rendered_lines:?}"
        );
        assert!(
            rendered_lines
                .iter()
                .any(|line| line.contains("Ran cargo test")),
            "expected expanded display to include second command: {rendered_lines:?}"
        );

        crate::exec_cell::set_exec_output_details_expanded(previous);
    }

    #[test]
    fn transcript_lines_expand_to_one_command_per_line() {
        let previous = crate::exec_cell::set_exec_output_details_expanded(true);

        let mut cell = ExecCell::new(
            ExecCall {
                call_id: "call-1".to_string(),
                command: vec!["bash".into(), "-lc".into(), "echo hello".into()],
                parsed: Vec::new(),
                output: Some(CommandOutput {
                    exit_code: 0,
                    formatted_output: "hello".to_string(),
                    aggregated_output: "hello".to_string(),
                }),
                source: ExecCommandSource::Agent,
                start_time: None,
                duration: Some(std::time::Duration::from_secs(1)),
                interaction_input: None,
            },
            false,
        );
        cell.calls.push(ExecCall {
            call_id: "call-2".to_string(),
            command: vec!["bash".into(), "-lc".into(), "cargo test".into()],
            parsed: Vec::new(),
            output: Some(CommandOutput {
                exit_code: 0,
                formatted_output: "ok".to_string(),
                aggregated_output: "ok".to_string(),
            }),
            source: ExecCommandSource::Agent,
            start_time: None,
            duration: Some(std::time::Duration::from_secs(2)),
            interaction_input: None,
        });

        let rendered = cell.transcript_lines(80);
        assert_eq!(
            rendered.len(),
            2,
            "expanded transcript should show one line per command"
        );

        let rendered_lines = rendered
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(
            rendered_lines
                .iter()
                .any(|line| line.contains("Ran echo hello")),
            "expected expanded transcript to include first command: {rendered_lines:?}"
        );
        assert!(
            rendered_lines
                .iter()
                .any(|line| line.contains("Ran cargo test")),
            "expected expanded transcript to include second command: {rendered_lines:?}"
        );

        crate::exec_cell::set_exec_output_details_expanded(previous);
    }
}

/*
   编号（如：1）：修改
   主要修改内容：将命令区默认态改为聚合摘要，并把展开态收紧为逐条命令一行摘要，不再渲染旧版大段输出详情。
   修改目的：让 Ctrl+O 的行为更稳定，默认视图更紧凑，同时保留对多条命令执行情况的可见性。

*/
