//! Structured analysis report generation.

extern crate alloc;
use alloc::format;
use alloc::string::String;

use crate::recovery::analyzer::AnalysisResult;

pub fn generate_json(analysis: &AnalysisResult) -> Result<String, String> {
    serde_json_core::to_string::<_, 4096>(analysis)
        .map(|s| String::from(s.as_str()))
        .map_err(|e| format!("serialization failed: {}", e))
}

pub fn generate_markdown(analysis: &AnalysisResult) -> String {
    let mut md = String::new();
    md.push_str("# Crash Analysis Report\n\n");
    md.push_str(&format!("**Event**: {}\n\n", analysis.crash_event));
    md.push_str(&format!("**Timestamp**: {}\n\n", analysis.timestamp));
    md.push_str(&format!("**Crash PC**: {:#018x}\n\n", analysis.crash_pc));
    if let Some(func) = &analysis.crash_function {
        let offset = analysis.crash_function_offset.map(|o| format!(" + {}", o)).unwrap_or_default();
        md.push_str(&format!("**Function**: `{func}{offset}`\n\n"));
    }
    md.push_str("## Summary\n\n");
    md.push_str(&analysis.summary);
    md.push_str("\n\n");
    if !analysis.possible_causes.is_empty() {
        md.push_str("## Possible Causes\n\n");
        for cause in &analysis.possible_causes {
            md.push_str(&format!("- {}\n", cause));
        }
        md.push_str("\n");
    }
    md.push_str("## Call Stack\n\n```\n");
    for (i, frame) in analysis.backtrace.iter().enumerate() {
        let func = frame.func_name.as_deref().unwrap_or("<unknown>");
        let offset = frame.func_offset.map(|o| format!("+{}", o)).unwrap_or_default();
        md.push_str(&format!("  #{:<3} {:#018x} {}{}\n", i, frame.pc, func, offset));
    }
    md.push_str("```\n\n## Process\n\n");
    md.push_str(&format!("- **Name**: {}\n", analysis.process.name));
    md.push_str(&format!("- **PID**: {:?}\n", analysis.process.pid));
    md.push_str(&format!("- **State**: {}\n", analysis.process.state));
    md.push_str(&format!("- **Kernel thread**: {}\n", analysis.process.is_kernel_thread));
    md.push_str("\n## Key Registers\n\n| Register | Value |\n|----------|-------|\n");
    for (name, val) in &analysis.key_registers {
        md.push_str(&format!("| {} | {:#018x} |\n", name, val));
    }

    // Data-structure sanity checks.
    // (dstruct_check field removed — not yet implemented for Linux targets)

    md
}

pub fn save_reports(analysis: &AnalysisResult, base_name: &str) -> Result<(String, String), String> {
    let json_path = format!("{}.json", base_name);
    let md_path = format!("{}.md", base_name);
    let json = generate_json(analysis)?;
    let md = generate_markdown(analysis);
    ax_std::fs::write(&json_path, json.as_bytes()).map_err(|e| format!("write {}: {}", json_path, e))?;
    ax_std::fs::write(&md_path, md.as_bytes()).map_err(|e| format!("write {}: {}", md_path, e))?;
    ax_std::println!("[report] saved JSON: {}", json_path);
    ax_std::println!("[report] saved MD: {}", md_path);
    Ok((json_path, md_path))
}
