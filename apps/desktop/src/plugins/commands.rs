#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChatCommand {
    pub name: &'static str,
    pub description: &'static str,
    pub insert_text: &'static str,
    pub requires_args: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatCommandResolution {
    NotCommand,
    Unknown,
    MissingArguments {
        message: String,
        insert_text: &'static str,
    },
    Resolved {
        display: String,
        prompt: String,
    },
}

pub const CHAT_COMMANDS: [ChatCommand; 4] = [
    ChatCommand {
        name: "/summary",
        description: "总结当前章节内容",
        insert_text: "/summary",
        requires_args: false,
    },
    ChatCommand {
        name: "/search",
        description: "搜索书籍内容并整理答案",
        insert_text: "/search ",
        requires_args: true,
    },
    ChatCommand {
        name: "/rewrite",
        description: "非持久改写当前章节正文",
        insert_text: "/rewrite ",
        requires_args: false,
    },
    ChatCommand {
        name: "/extract",
        description: "提取当前章节关键概念",
        insert_text: "/extract",
        requires_args: false,
    },
];

pub fn chat_command_suggestions(input: &str) -> Vec<ChatCommand> {
    let input = input.trim_start();
    let Some(token) = command_token(input) else {
        return Vec::new();
    };
    CHAT_COMMANDS
        .into_iter()
        .filter(|command| command.name.starts_with(token))
        .filter(|command| command.name != token)
        .collect()
}

pub fn resolve_chat_command(input: &str) -> ChatCommandResolution {
    let input = input.trim();
    if !input.starts_with('/') {
        return ChatCommandResolution::NotCommand;
    }
    let (name, args) = input
        .split_once(char::is_whitespace)
        .map_or((input, ""), |(name, args)| (name, args.trim()));
    let Some(command) = CHAT_COMMANDS
        .into_iter()
        .find(|command| command.name.eq_ignore_ascii_case(name))
    else {
        return ChatCommandResolution::Unknown;
    };
    if command.requires_args && args.is_empty() {
        let message = match command.name {
            "/search" => "请输入搜索关键词，例如 `/search feedback loops`。",
            _ => "这个技能需要补充参数。",
        };
        return ChatCommandResolution::MissingArguments {
            message: message.into(),
            insert_text: command.insert_text,
        };
    }

    let prompt = match command.name {
        "/summary" => "请总结当前章节内容。要求：用中文回答；先给出一句话概括，再列出关键要点；如果章节中有重要术语，请单独解释。".into(),
        "/search" => format!(
            "请在本书中搜索与“{args}”相关的信息，优先使用 searchBook；需要阅读完整章节时使用 getContent。请用中文回答，列出最相关的章节或段落，并简要解释上下文。"
        ),
        "/rewrite" => {
            let extra = if args.is_empty() {
                String::new()
            } else {
                format!("\n额外改写要求：{args}")
            };
            format!(
                "请改写当前章节正文，默认改成更通俗易懂的中文。必须先调用 getContent 获取 blockId，再调用 rewriteBlocks 修改实际渲染文本，不要只在回答中贴改写结果。保留原文核心信息、术语和逻辑；不要修改图片或表格；完成后只简要说明已改写完成。{extra}"
            )
        }
        "/extract" => "请提取当前章节的关键概念。要求：用中文回答；先列出概念清单，再分别解释每个概念的含义、它在本章中的作用，以及概念之间的关系。涉及本章具体内容时说明对应段落依据。".into(),
        _ => unreachable!("every registered command has a prompt"),
    };
    ChatCommandResolution::Resolved {
        display: input.to_owned(),
        prompt,
    }
}

fn command_token(input: &str) -> Option<&str> {
    input
        .starts_with('/')
        .then(|| input.split_whitespace().next().unwrap_or(input))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_match_slash_prefix_without_story_memory_commands() {
        let names = chat_command_suggestions("/s")
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["/summary", "/search"]);
        assert!(CHAT_COMMANDS.iter().all(|command| !matches!(
            command.name,
            "/story-index" | "/timeline" | "/profile" | "/relations" | "/entities"
        )));
        assert!(chat_command_suggestions("/summary").is_empty());
    }

    #[test]
    fn search_requires_arguments_and_expands_to_a_tool_prompt() {
        assert!(matches!(
            resolve_chat_command("/search"),
            ChatCommandResolution::MissingArguments { .. }
        ));
        let ChatCommandResolution::Resolved { display, prompt } =
            resolve_chat_command("/SEARCH feedback loops")
        else {
            panic!("command should resolve");
        };
        assert_eq!(display, "/SEARCH feedback loops");
        assert!(prompt.contains("feedback loops"));
        assert!(prompt.contains("searchBook"));
    }

    #[test]
    fn rewrite_expands_to_controlled_document_tools() {
        let ChatCommandResolution::Resolved { prompt, .. } =
            resolve_chat_command("/rewrite 保持英文")
        else {
            panic!("command should resolve");
        };
        assert!(prompt.contains("getContent"));
        assert!(prompt.contains("rewriteBlocks"));
        assert!(prompt.contains("保持英文"));
    }
}
