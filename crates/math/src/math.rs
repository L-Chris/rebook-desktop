use crate::fonts::TextMeasure;
use latex2mathml::{DisplayStyle, latex_to_mathml};
use quick_xml::events::Event as XmlEvent;
use quick_xml::reader::Reader as XmlReader;

#[derive(Debug)]
enum MathNode {
    Row(Vec<MathNode>),
    Ident(String),
    Number(String),
    Operator(String),
    Text(String),
    Sup {
        base: Box<MathNode>,
        sup: Box<MathNode>,
    },
    Sub {
        base: Box<MathNode>,
        sub: Box<MathNode>,
    },
    SubSup {
        base: Box<MathNode>,
        sub: Box<MathNode>,
        sup: Box<MathNode>,
    },
    Frac {
        num: Box<MathNode>,
        den: Box<MathNode>,
        line_thickness: Option<f32>, // None = default, Some(0.0) = no line
    },
    Sqrt {
        radicand: Box<MathNode>,
    },
    Root {
        radicand: Box<MathNode>,
        index: Box<MathNode>,
    },
    UnderOver {
        base: Box<MathNode>,
        under: Option<Box<MathNode>>,
        over: Option<Box<MathNode>>,
    },
    Space(f32),
    /// Table for matrices, cases, aligned equations
    Table {
        rows: Vec<Vec<MathNode>>,
        column_align: Vec<String>, // "left", "center", "right"
    },
    /// Stretchy operator (parentheses, brackets that scale)
    StretchyOp {
        op: String,
        form: String, // "prefix", "postfix", "infix"
    },
}

pub struct MathResult {
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
    pub svg_fragment: String,
}

pub fn render_math<T: TextMeasure>(
    latex: &str,
    font_size: f32,
    text_color: &str,
    measure: &mut T,
    display: bool,
) -> Result<MathResult, String> {
    render_math_at(latex, font_size, text_color, measure, display, 0.0, 0.0)
}

pub fn render_math_at<T: TextMeasure>(
    latex: &str,
    font_size: f32,
    text_color: &str,
    measure: &mut T,
    display: bool,
    x: f32,
    baseline_y: f32,
) -> Result<MathResult, String> {
    let style = if display {
        DisplayStyle::Block
    } else {
        DisplayStyle::Inline
    };

    let mathml =
        latex_to_mathml(latex, style).map_err(|error| format!("LaTeX parse error: {error:?}"))?;

    let root = parse_mathml(&mathml)?;
    let mbox = layout_node(&root, font_size, text_color, measure, x, baseline_y);

    Ok(MathResult {
        width: mbox.width,
        ascent: mbox.ascent,
        descent: mbox.descent,
        svg_fragment: mbox.svg,
    })
}

struct MathBox {
    width: f32,
    ascent: f32,
    descent: f32,
    svg: String,
}

type Attrs = Vec<(String, String)>;

#[allow(
    clippy::too_many_lines,
    reason = "faithful MathML state-machine parser"
)]
fn parse_mathml(mathml: &str) -> Result<MathNode, String> {
    let mut reader = XmlReader::from_str(mathml);
    reader.config_mut().trim_text(true);

    // Stack now stores: (tag_name, children, attributes)
    let mut stack: Vec<(String, Vec<MathNode>, Attrs)> = Vec::new();
    let mut buf = Vec::new();

    // Track table row context for mtable parsing
    let mut current_table_rows: Vec<Vec<MathNode>> = Vec::new();
    let mut current_row_cells: Vec<MathNode> = Vec::new();
    let mut in_table = 0i32; // nesting counter
    let mut in_row = 0i32;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(XmlEvent::Start(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let attrs: Attrs = e
                    .attributes()
                    .filter_map(Result::ok)
                    .map(|a| {
                        (
                            String::from_utf8_lossy(a.key.as_ref()).to_string(),
                            String::from_utf8_lossy(&a.value).to_string(),
                        )
                    })
                    .collect();

                if name == "mtable" {
                    in_table += 1;
                    if in_table == 1 {
                        current_table_rows.clear();
                    }
                } else if name == "mtr" && in_table == 1 {
                    in_row += 1;
                    current_row_cells.clear();
                } else if name == "mtd" && in_table == 1 && in_row == 1 {
                    // mtd content goes on stack
                }

                stack.push((name, Vec::new(), attrs));
            }
            Ok(XmlEvent::Text(ref e)) => {
                let text = e.decode().unwrap_or_default().to_string();
                if !text.is_empty()
                    && let Some((_, children, _)) = stack.last_mut()
                {
                    children.push(MathNode::Text(text));
                }
            }
            Ok(XmlEvent::End(ref e)) => {
                let _name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if let Some((tag, children, attrs)) = stack.pop() {
                    // Handle table elements specially
                    if tag == "mtd" && in_table == 1 && in_row == 1 {
                        let cell = if children.len() == 1 {
                            children.into_iter().next().unwrap()
                        } else {
                            MathNode::Row(children)
                        };
                        current_row_cells.push(cell);
                    } else if tag == "mtr" && in_table == 1 {
                        in_row -= 1;
                        if !current_row_cells.is_empty() {
                            current_table_rows.push(std::mem::take(&mut current_row_cells));
                        }
                    } else if tag == "mtable" && in_table == 1 {
                        in_table -= 1;
                        let table_node = MathNode::Table {
                            rows: std::mem::take(&mut current_table_rows),
                            column_align: vec!["center".to_string()], // default center
                        };
                        if let Some((_, parent_children, _)) = stack.last_mut() {
                            parent_children.push(table_node);
                        } else {
                            return Ok(table_node);
                        }
                    } else {
                        let node = build_node(&tag, children, &attrs);
                        if let Some((_, parent_children, _)) = stack.last_mut() {
                            parent_children.push(node);
                        } else {
                            return Ok(node);
                        }
                    }
                }
            }
            Ok(XmlEvent::Empty(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let attrs: Attrs = e
                    .attributes()
                    .filter_map(Result::ok)
                    .map(|a| {
                        (
                            String::from_utf8_lossy(a.key.as_ref()).to_string(),
                            String::from_utf8_lossy(&a.value).to_string(),
                        )
                    })
                    .collect();

                if name == "mspace" {
                    let mut width_em = 0.0;
                    for (key, val) in &attrs {
                        if key == "width"
                            && let Some(stripped) = val.strip_suffix("em")
                        {
                            width_em = stripped.parse().unwrap_or(0.0);
                        }
                    }
                    let node = MathNode::Space(width_em);
                    if let Some((_, parent_children, _)) = stack.last_mut() {
                        parent_children.push(node);
                    }
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(error) => return Err(format!("XML parse error: {error}")),
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::Row(Vec::new()))
}

fn get_attr(attrs: &[(String, String)], name: &str) -> Option<String> {
    attrs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
}

#[allow(
    clippy::too_many_lines,
    reason = "one match arm per supported MathML node"
)]
fn build_node(tag: &str, mut children: Vec<MathNode>, attrs: &Attrs) -> MathNode {
    match tag {
        "mi" => {
            let text = extract_text(&children);
            MathNode::Ident(text)
        }
        "mn" => {
            let text = extract_text(&children);
            MathNode::Number(text)
        }
        "mo" => {
            let text = extract_text(&children);
            // Check for stretchy attribute
            let stretchy = get_attr(attrs, "stretchy").is_some_and(|value| value == "true");
            let form = get_attr(attrs, "form").unwrap_or_else(|| "infix".to_string());

            if stretchy && !text.is_empty() {
                MathNode::StretchyOp { op: text, form }
            } else {
                MathNode::Operator(text)
            }
        }
        "mtext" => {
            let text = extract_text(&children);
            MathNode::Text(text)
        }
        "msup" if children.len() >= 2 => {
            let sup = children.pop().unwrap();
            let base = children.pop().unwrap();
            MathNode::Sup {
                base: Box::new(base),
                sup: Box::new(sup),
            }
        }
        "msub" if children.len() >= 2 => {
            let sub = children.pop().unwrap();
            let base = children.pop().unwrap();
            MathNode::Sub {
                base: Box::new(base),
                sub: Box::new(sub),
            }
        }
        "msubsup" if children.len() >= 3 => {
            let sup = children.pop().unwrap();
            let sub = children.pop().unwrap();
            let base = children.pop().unwrap();
            MathNode::SubSup {
                base: Box::new(base),
                sub: Box::new(sub),
                sup: Box::new(sup),
            }
        }
        "mfrac" if children.len() >= 2 => {
            let den = children.pop().unwrap();
            let num = children.pop().unwrap();
            // Check for linethickness attribute (used by binomial)
            let line_thickness = get_attr(attrs, "linethickness").and_then(|v| {
                if v == "0" {
                    Some(0.0)
                } else {
                    v.parse::<f32>().ok()
                }
            });
            MathNode::Frac {
                num: Box::new(num),
                den: Box::new(den),
                line_thickness,
            }
        }
        "msqrt" => {
            let radicand = if children.len() == 1 {
                children.pop().unwrap()
            } else {
                MathNode::Row(children)
            };
            MathNode::Sqrt {
                radicand: Box::new(radicand),
            }
        }
        "mroot" if children.len() >= 2 => {
            // Note: in MathML mroot, the index comes AFTER the radicand
            let index = children.pop().unwrap();
            let radicand = children.pop().unwrap();
            MathNode::Root {
                radicand: Box::new(radicand),
                index: Box::new(index),
            }
        }
        "mover" if children.len() >= 2 => {
            let over = children.pop().unwrap();
            let base = children.pop().unwrap();
            MathNode::UnderOver {
                base: Box::new(base),
                under: None,
                over: Some(Box::new(over)),
            }
        }
        "munder" if children.len() >= 2 => {
            let under = children.pop().unwrap();
            let base = children.pop().unwrap();
            MathNode::UnderOver {
                base: Box::new(base),
                under: Some(Box::new(under)),
                over: None,
            }
        }
        "munderover" if children.len() >= 3 => {
            let over = children.pop().unwrap();
            let under = children.pop().unwrap();
            let base = children.pop().unwrap();
            MathNode::UnderOver {
                base: Box::new(base),
                under: Some(Box::new(under)),
                over: Some(Box::new(over)),
            }
        }
        _ => {
            if children.len() == 1 {
                children.pop().unwrap()
            } else {
                MathNode::Row(children)
            }
        }
    }
}

fn extract_text(children: &[MathNode]) -> String {
    let mut s = String::new();
    for child in children {
        match child {
            MathNode::Text(t)
            | MathNode::Ident(t)
            | MathNode::Number(t)
            | MathNode::Operator(t) => s.push_str(t),
            _ => {}
        }
    }
    s
}

fn escape_xml(text: &str) -> String {
    crate::xml::escape_xml(text)
}

fn measure_token<T: TextMeasure>(
    text: &str,
    font_size: f32,
    italic: bool,
    measure: &mut T,
) -> (f32, f32) {
    let cleaned = crate::xml::sanitize_xml_text(text);
    measure.measure_text(&cleaned, font_size, false, false, italic, None)
}

#[allow(
    clippy::too_many_lines,
    reason = "recursive layout for each MathML node type"
)]
fn layout_node<T: TextMeasure>(
    node: &MathNode,
    font_size: f32,
    color: &str,
    measure: &mut T,
    x: f32,
    baseline_y: f32,
) -> MathBox {
    match node {
        MathNode::Ident(text) => {
            let italic = text.len() == 1 && text.chars().next().is_some_and(char::is_alphabetic);
            let (w, _h) = measure_token(text, font_size, italic, measure);
            let style = if italic { " font-style=\"italic\"" } else { "" };
            let svg = format!(
                r#"<text x="{:.2}" y="{:.2}" font-family="serif" font-size="{:.2}" fill="{}"{}>{}</text>"#,
                x,
                baseline_y,
                font_size,
                color,
                style,
                escape_xml(text)
            );
            MathBox {
                width: w,
                ascent: font_size * 0.75,
                descent: font_size * 0.25,
                svg,
            }
        }
        MathNode::Number(text) => {
            let (w, _h) = measure_token(text, font_size, false, measure);
            let svg = format!(
                r#"<text x="{:.2}" y="{:.2}" font-family="serif" font-size="{:.2}" fill="{}">{}</text>"#,
                x,
                baseline_y,
                font_size,
                color,
                escape_xml(text)
            );
            MathBox {
                width: w,
                ascent: font_size * 0.75,
                descent: font_size * 0.25,
                svg,
            }
        }
        MathNode::Operator(text) => {
            let is_large = is_large_operator(text);
            let effective_size = if is_large { font_size * 1.4 } else { font_size };
            let (w, _h) = measure_token(text, effective_size, false, measure);
            let spacing = font_size * 0.15;
            let total_w = w + spacing * 2.0;
            let y_offset = if is_large {
                baseline_y + (effective_size - font_size) * 0.2
            } else {
                baseline_y
            };
            let svg = format!(
                r#"<text x="{:.2}" y="{:.2}" font-family="serif" font-size="{:.2}" fill="{}">{}</text>"#,
                x + spacing,
                y_offset,
                effective_size,
                color,
                escape_xml(text)
            );
            MathBox {
                width: total_w,
                ascent: if is_large {
                    effective_size * 0.8
                } else {
                    font_size * 0.75
                },
                descent: if is_large {
                    effective_size * 0.3
                } else {
                    font_size * 0.25
                },
                svg,
            }
        }
        MathNode::Text(text) => {
            let (w, _h) = measure_token(text, font_size, false, measure);
            let svg = format!(
                r#"<text x="{:.2}" y="{:.2}" font-family="sans-serif" font-size="{:.2}" fill="{}">{}</text>"#,
                x,
                baseline_y,
                font_size,
                color,
                escape_xml(text)
            );
            MathBox {
                width: w,
                ascent: font_size * 0.75,
                descent: font_size * 0.25,
                svg,
            }
        }
        MathNode::Space(em) => MathBox {
            width: font_size * em,
            ascent: 0.0,
            descent: 0.0,
            svg: String::new(),
        },
        MathNode::Row(children) => layout_row(children, font_size, color, measure, x, baseline_y),
        MathNode::Sup { base, sup } => {
            let base_box = layout_node(base, font_size, color, measure, x, baseline_y);

            let sup_size = font_size * 0.7;
            let sup_y = baseline_y - base_box.ascent * 0.55;
            let sup_box = layout_node(sup, sup_size, color, measure, x + base_box.width, sup_y);

            let total_width = base_box.width + sup_box.width;
            let ascent = base_box.ascent.max(sup_box.ascent + base_box.ascent * 0.55);
            let descent = base_box.descent;

            MathBox {
                width: total_width,
                ascent,
                descent,
                svg: format!("{}{}", base_box.svg, sup_box.svg),
            }
        }
        MathNode::Sub { base, sub } => {
            let base_box = layout_node(base, font_size, color, measure, x, baseline_y);

            let sub_size = font_size * 0.7;
            let sub_y = baseline_y + base_box.descent + sub_size * 0.35;
            let sub_box = layout_node(sub, sub_size, color, measure, x + base_box.width, sub_y);

            let total_width = base_box.width + sub_box.width;
            let ascent = base_box.ascent;
            let descent =
                (base_box.descent + sub_size * 0.35 + sub_box.descent).max(base_box.descent);

            MathBox {
                width: total_width,
                ascent,
                descent,
                svg: format!("{}{}", base_box.svg, sub_box.svg),
            }
        }
        MathNode::SubSup { base, sub, sup } => {
            let base_box = layout_node(base, font_size, color, measure, x, baseline_y);

            let script_size = font_size * 0.7;

            let superscript_y = baseline_y - base_box.ascent * 0.55;
            let superscript = layout_node(
                sup,
                script_size,
                color,
                measure,
                x + base_box.width,
                superscript_y,
            );

            let subscript_y = baseline_y + base_box.descent + script_size * 0.35;
            let subscript = layout_node(
                sub,
                script_size,
                color,
                measure,
                x + base_box.width,
                subscript_y,
            );

            let script_width = superscript.width.max(subscript.width);
            let total_width = base_box.width + script_width;
            let ascent = base_box
                .ascent
                .max(superscript.ascent + base_box.ascent * 0.55);
            let descent =
                (base_box.descent + script_size * 0.35 + subscript.descent).max(base_box.descent);

            MathBox {
                width: total_width,
                ascent,
                descent,
                svg: format!("{}{}{}", base_box.svg, superscript.svg, subscript.svg),
            }
        }
        MathNode::UnderOver { base, under, over } => layout_underover(
            base,
            under.as_deref(),
            over.as_deref(),
            font_size,
            color,
            measure,
            x,
            baseline_y,
        ),
        MathNode::Frac {
            num,
            den,
            line_thickness,
        } => {
            let frac_size = font_size * 0.85;

            let num_box = layout_node(num, frac_size, color, measure, 0.0, 0.0);
            let den_box = layout_node(den, frac_size, color, measure, 0.0, 0.0);

            let max_width = num_box.width.max(den_box.width);
            let padding = font_size * 0.2;
            let frac_width = max_width + padding * 2.0;

            let rule_y = baseline_y - font_size * 0.3;
            let gap = font_size * 0.15;

            let num_baseline = rule_y - gap - num_box.descent;
            let den_baseline = rule_y + gap + den_box.ascent;

            let num_x = x + (frac_width - num_box.width) / 2.0;
            let den_x = x + (frac_width - den_box.width) / 2.0;

            let num_rendered = layout_node(num, frac_size, color, measure, num_x, num_baseline);
            let den_rendered = layout_node(den, frac_size, color, measure, den_x, den_baseline);

            // Only draw line if line_thickness is not Some(0.0) (for binomials)
            let rule_svg = if line_thickness == &Some(0.0) {
                String::new()
            } else {
                format!(
                    r#"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="{}" stroke-width="1" />"#,
                    x,
                    rule_y,
                    x + frac_width,
                    rule_y,
                    color
                )
            };

            let ascent = (baseline_y - num_baseline + num_rendered.ascent).max(font_size * 0.75);
            let descent = (den_baseline - baseline_y + den_rendered.descent).max(font_size * 0.25);

            MathBox {
                width: frac_width,
                ascent,
                descent,
                svg: format!("{}{}{}", num_rendered.svg, rule_svg, den_rendered.svg),
            }
        }
        MathNode::Sqrt { radicand } => {
            let inner = layout_node(radicand, font_size, color, measure, 0.0, 0.0);

            let radical_width = font_size * 0.6;
            let padding = font_size * 0.1;
            let overbar_gap = font_size * 0.15;
            let total_width = radical_width + inner.width + padding;

            let inner_box = layout_node(
                radicand,
                font_size,
                color,
                measure,
                x + radical_width,
                baseline_y,
            );

            let top_y = baseline_y - inner_box.ascent - overbar_gap;
            let bottom_y = baseline_y + inner_box.descent;

            let radical_svg = format!(
                r#"<path d="M {:.2} {:.2} L {:.2} {:.2} L {:.2} {:.2} L {:.2} {:.2}" stroke="{}" stroke-width="1.2" fill="none" />"#,
                x,
                baseline_y - font_size * 0.15,
                x + radical_width * 0.35,
                baseline_y,
                x + radical_width * 0.6,
                top_y,
                x + radical_width + inner_box.width + padding,
                top_y,
                color
            );

            let ascent = (baseline_y - top_y).max(inner_box.ascent + overbar_gap);
            let descent = inner_box.descent.max(bottom_y - baseline_y);

            MathBox {
                width: total_width,
                ascent,
                descent,
                svg: format!("{}{}", radical_svg, inner_box.svg),
            }
        }
        MathNode::Root { radicand, index } => {
            let inner = layout_node(radicand, font_size, color, measure, 0.0, 0.0);
            let index_size = font_size * 0.6;

            let radical_width = font_size * 0.6;
            let index_width = font_size * 0.5;
            let padding = font_size * 0.1;
            let overbar_gap = font_size * 0.15;
            let total_width = index_width + radical_width + inner.width + padding;

            let inner_box = layout_node(
                radicand,
                font_size,
                color,
                measure,
                x + index_width + radical_width,
                baseline_y,
            );

            let top_y = baseline_y - inner_box.ascent - overbar_gap;
            let bottom_y = baseline_y + inner_box.descent;

            // Render the index (nth root degree) in the notch
            let index_baseline = baseline_y - inner_box.ascent * 0.3;
            let index_box = layout_node(index, index_size, color, measure, x, index_baseline);

            // Radical symbol with notch for index
            let radical_svg = format!(
                r#"<path d="M {:.2} {:.2} L {:.2} {:.2} L {:.2} {:.2} L {:.2} {:.2} L {:.2} {:.2}" stroke="{}" stroke-width="1.2" fill="none" />"#,
                x + index_width,
                baseline_y - font_size * 0.15,
                x + index_width + radical_width * 0.35,
                baseline_y,
                x + index_width + radical_width * 0.6,
                top_y,
                x + index_width + radical_width + inner_box.width + padding,
                top_y,
                x + index_width + radical_width + inner_box.width + padding - font_size * 0.1,
                top_y - font_size * 0.05,
                color
            );

            let ascent = (baseline_y - top_y).max(inner_box.ascent + overbar_gap);
            let descent = inner_box.descent.max(bottom_y - baseline_y);

            MathBox {
                width: total_width,
                ascent,
                descent,
                svg: format!("{}{}{}", index_box.svg, radical_svg, inner_box.svg),
            }
        }
        MathNode::Table { rows, column_align } => {
            layout_table(rows, column_align, font_size, color, measure, x, baseline_y)
        }
        MathNode::StretchyOp { op, form } => {
            let (w, _h) = measure_token(op, font_size, false, measure);
            let offset = match form.as_str() {
                "prefix" => font_size * 0.08,
                "postfix" => -font_size * 0.08,
                _ => 0.0,
            };
            let svg = format!(
                r#"<text x="{:.2}" y="{:.2}" font-family="serif" font-size="{:.2}" fill="{}">{}</text>"#,
                x + offset,
                baseline_y,
                font_size,
                color,
                escape_xml(op)
            );
            MathBox {
                width: w,
                ascent: font_size * 0.75,
                descent: font_size * 0.25,
                svg,
            }
        }
    }
}

fn layout_row<T: TextMeasure>(
    children: &[MathNode],
    font_size: f32,
    color: &str,
    measure: &mut T,
    start_x: f32,
    baseline_y: f32,
) -> MathBox {
    let mut cx = start_x;
    let mut svg = String::new();
    let mut max_ascent: f32 = font_size * 0.75;
    let mut max_descent: f32 = font_size * 0.25;

    for child in children {
        let child_box = layout_node(child, font_size, color, measure, cx, baseline_y);
        max_ascent = max_ascent.max(child_box.ascent);
        max_descent = max_descent.max(child_box.descent);
        cx += child_box.width;
        svg.push_str(&child_box.svg);
    }

    MathBox {
        width: cx - start_x,
        ascent: max_ascent,
        descent: max_descent,
        svg,
    }
}

#[allow(clippy::too_many_arguments, reason = "recursive MathML layout inputs")]
fn layout_underover<T: TextMeasure>(
    base: &MathNode,
    under: Option<&MathNode>,
    over: Option<&MathNode>,
    font_size: f32,
    color: &str,
    measure: &mut T,
    x: f32,
    baseline_y: f32,
) -> MathBox {
    let base_box = layout_node(base, font_size, color, measure, 0.0, 0.0);
    let script_size = font_size * 0.65;
    let gap = font_size * 0.15;

    let over_box = over.map(|o| layout_node(o, script_size, color, measure, 0.0, 0.0));
    let under_box = under.map(|u| layout_node(u, script_size, color, measure, 0.0, 0.0));

    let max_width = [
        base_box.width,
        over_box.as_ref().map_or(0.0, |b| b.width),
        under_box.as_ref().map_or(0.0, |b| b.width),
    ]
    .into_iter()
    .fold(0.0f32, f32::max);

    let mut svg = String::new();
    let mut total_ascent = base_box.ascent;
    let mut total_descent = base_box.descent;

    let base_x = x + (max_width - base_box.width) / 2.0;
    let base_rendered = layout_node(base, font_size, color, measure, base_x, baseline_y);
    svg.push_str(&base_rendered.svg);

    if let (Some(over_node), Some(ob)) = (over, &over_box) {
        let over_baseline = baseline_y - base_box.ascent - gap - ob.descent;
        let over_x = x + (max_width - ob.width) / 2.0;
        let over_rendered = layout_node(
            over_node,
            script_size,
            color,
            measure,
            over_x,
            over_baseline,
        );
        svg.push_str(&over_rendered.svg);
        total_ascent = base_box.ascent + gap + ob.ascent + ob.descent;
    }

    if let (Some(under_node), Some(ub)) = (under, &under_box) {
        let under_baseline = baseline_y + base_box.descent + gap + ub.ascent;
        let under_x = x + (max_width - ub.width) / 2.0;
        let under_rendered = layout_node(
            under_node,
            script_size,
            color,
            measure,
            under_x,
            under_baseline,
        );
        svg.push_str(&under_rendered.svg);
        total_descent = base_box.descent + gap + ub.ascent + ub.descent;
    }

    MathBox {
        width: max_width,
        ascent: total_ascent,
        descent: total_descent,
        svg,
    }
}

#[allow(
    clippy::cast_precision_loss,
    reason = "table dimensions are bounded by parsed content"
)]
fn layout_table<T: TextMeasure>(
    rows: &[Vec<MathNode>],
    _column_align: &[String],
    font_size: f32,
    color: &str,
    measure: &mut T,
    x: f32,
    baseline_y: f32,
) -> MathBox {
    if rows.is_empty() {
        return MathBox {
            width: 0.0,
            ascent: font_size * 0.75,
            descent: font_size * 0.25,
            svg: String::new(),
        };
    }

    let cell_size = font_size * 0.9;
    let row_gap = font_size * 0.3;
    let col_gap = font_size * 0.4;

    // First pass: measure all cells to determine column widths and row heights
    let mut col_widths: Vec<f32> = Vec::new();
    let mut row_heights: Vec<(f32, f32)> = Vec::new(); // (ascent, descent) per row

    for row in rows {
        let mut row_ascent = cell_size * 0.75;
        let mut row_descent = cell_size * 0.25;

        for (col_idx, cell) in row.iter().enumerate() {
            let cell_box = layout_node(cell, cell_size, color, measure, 0.0, 0.0);

            // Expand column width if needed
            while col_widths.len() <= col_idx {
                col_widths.push(0.0);
            }
            col_widths[col_idx] = col_widths[col_idx].max(cell_box.width);

            row_ascent = row_ascent.max(cell_box.ascent);
            row_descent = row_descent.max(cell_box.descent);
        }
        row_heights.push((row_ascent, row_descent));
    }

    // Calculate total table dimensions
    let total_width: f32 =
        col_widths.iter().sum::<f32>() + col_gap * (col_widths.len().max(1) - 1) as f32;
    let total_height: f32 =
        row_heights.iter().map(|(a, d)| a + d).sum::<f32>() + row_gap * (rows.len() - 1) as f32;

    // Center the table vertically around baseline
    let table_top = baseline_y - total_height / 2.0;

    // Second pass: render all cells
    let mut svg = String::new();
    let mut current_y = table_top;

    for (row_idx, row) in rows.iter().enumerate() {
        let (row_ascent, row_descent) = row_heights[row_idx];
        let row_baseline = current_y + row_ascent;
        let mut current_x = x;

        for (col_idx, cell) in row.iter().enumerate() {
            let col_width = col_widths.get(col_idx).copied().unwrap_or(0.0);
            let cell_box = layout_node(cell, cell_size, color, measure, 0.0, 0.0);

            // Center cell in column
            let cell_x = current_x + (col_width - cell_box.width) / 2.0;

            let rendered = layout_node(cell, cell_size, color, measure, cell_x, row_baseline);
            svg.push_str(&rendered.svg);

            current_x += col_width + col_gap;
        }

        current_y += row_ascent + row_descent + row_gap;
    }

    MathBox {
        width: total_width,
        ascent: total_height / 2.0,
        descent: total_height / 2.0,
        svg,
    }
}

fn is_large_operator(text: &str) -> bool {
    matches!(
        text,
        "∑" | "∏"
            | "∐"
            | "⋀"
            | "⋁"
            | "⋂"
            | "⋃"
            | "∫"
            | "∬"
            | "∭"
            | "∮"
            | "⨁"
            | "⨂"
            | "⨀"
    )
}
