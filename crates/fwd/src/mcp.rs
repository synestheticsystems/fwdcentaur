//! The daemon's MCP endpoint: fwd's commands as Model Context Protocol tools, served over
//! Streamable HTTP at `/mcp` on the daemon's own listener (crates/fwd/src/serve.rs), so that a
//! Codex thread works in the task graph without a shell (task jDv1EY7K). It is the CLI by another
//! door, not a second fwd API: each tool is a CLI command (`tools`), whose arguments are parsed by
//! the CLI's own parser and run by the CLI's own dispatcher (`crate::run`) over the daemon's durable
//! node, with the CLI's refusals before it (`crate::refuse_by_identity`) and its bookkeeping after
//! it (`crate::run_as_caller`), its answer captured rather than printed (`crate::Output`).
//!
//! Who calls is never an argument. The transport says it ([`caller::Caller`], from the metadata
//! Codex attaches to each tool call), and the operations ([`Operations`]) take it as given, so that
//! a hosted endpoint can hand them an authenticated caller later without changing them; the local
//! endpoint asks for no login, under the same local trust as the rest of the daemon. A call fwd
//! cannot attribute is refused. The first call that needs an actor in a project, a write or a read
//! of the caller's own inbox or tasks, makes the agent bound to the thread there, once, however many
//! such calls arrive together ([`Operations::attach`]); a thread that names itself first with
//! `principal_add` is made as it says. Calls that discover projects (`project_list`) or say who calls
//! (`whoami`) make no one.

pub mod caller;
pub mod tools;

use crate::{App, Captured, Command, Format, Output, PrincipalCommand};
use ankurah::{Context, EntityId};
use anyhow::{anyhow, bail, Context as _, Result};
use caller::Caller;
use clap::{CommandFactory, FromArgMatches};
use fwd_core::ops;
use fwd_core::project::{Paths, Project};
use fwd_core::subject;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, InitializeResult,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use uuid::Uuid;

/// What the endpoint needs of the daemon: the data folder, its port, and the projects it hosts,
/// each with a context on its durable node. The daemon gives it (serve.rs); a hosted server would
/// give its own.
pub trait Hosting: Send + Sync + 'static {
    fn paths(&self) -> &Paths;
    fn port(&self) -> u16;
    /// Every project hosted, by id and name, with why it could not open when it could not.
    fn projects(&self) -> Vec<(Uuid, String, Option<String>)>;
    /// The project with this id, and a context on its node, when it is open.
    fn open(&self, id: Uuid) -> Option<(Project, Context)>;
}

/// What the daemon does for a caller it attributed a call to, beyond the graph: the place where
/// registering a thread for its inbox, and waking it when a notification arrives, will go
/// (another task builds them). Called after every call that acted as a principal, with the thread
/// that made it, so that a registration stays fresh; the default does nothing.
pub trait Seen: Send + Sync + 'static {
    fn seen(&self, _project: Uuid, _principal: EntityId, _caller: &Caller) {}
}

/// The [`Seen`] that does nothing, until inbox registration and wake delivery are built.
pub struct NoWake;
impl Seen for NoWake {}

/// The protocol-free half of the endpoint: the tools, and running one for a caller.
pub struct Operations {
    hosting: Arc<dyn Hosting>,
    seen: Arc<dyn Seen>,
    tools: Vec<tools::Spec>,
    /// Held while a caller's principal is looked for and made, so that two first calls of one
    /// thread arriving together make one principal: the second finds the first's.
    attaching: tokio::sync::Mutex<()>,
}

/// What a tool call answered: what the command printed, and what it said beside it, or why it was
/// refused. A command that failed after it had made something (its stamp, or the notice after it,
/// failed) keeps what it printed in `made`, as the CLI prints the record before its error, so that
/// a caller told not to run it again still learns the id of what it made.
#[derive(Debug)]
pub struct Outcome {
    pub result: std::result::Result<Value, String>,
    pub notes: Vec<String>,
    pub made: Option<Value>,
}

impl Outcome {
    pub fn refused(why: impl Into<String>) -> Outcome {
        Outcome { result: Err(why.into()), notes: Vec::new(), made: None }
    }
}

/// What a captured command printed, as one value: none, the one, or a list of several.
fn printed(mut values: Vec<Value>) -> Value {
    match values.len() {
        0 => Value::Null,
        1 => values.remove(0),
        _ => Value::Array(values),
    }
}

/// The discovery tool: which projects the daemon hosts, to choose one before anything is made.
const PROJECT_LIST: &str = "project_list";
/// Who the caller is, and in a project the principal it acts as.
const WHOAMI: &str = "whoami";
/// The CLI's full help for a tool's command, which the catalog's descriptions abridge.
const HELP: &str = "help";

impl Operations {
    pub fn new(hosting: Arc<dyn Hosting>, seen: Arc<dyn Seen>) -> Operations {
        Operations { hosting, seen, tools: tools::specs(), attaching: tokio::sync::Mutex::new(()) }
    }

    /// Every tool, as `tools/list` gives it.
    pub fn tools(&self) -> Vec<Tool> {
        catalog(&self.tools)
    }

    /// Whether a tool of this name exists.
    pub fn has(&self, name: &str) -> bool {
        name == PROJECT_LIST || name == WHOAMI || name == HELP || self.tools.iter().any(|t| t.name == name)
    }

    /// Runs the tool `name` for `caller` with `args`.
    pub async fn call(&self, caller: &Caller, name: &str, args: &Map<String, Value>) -> Outcome {
        let mut captured = Captured::default();
        let ran = match name {
            PROJECT_LIST => self.project_list(args),
            WHOAMI => self.whoami(caller, args).await,
            HELP => help(args),
            _ => match self.tools.iter().find(|t| t.name == name) {
                Some(spec) => self
                    .command(caller, spec, args, &mut captured)
                    .await
                    .map(|()| printed(std::mem::take(&mut captured.values))),
                None => Err(anyhow!("no tool is named `{name}`")),
            },
        };
        let Captured { values, notes } = captured;
        match ran {
            Ok(value) => Outcome { result: Ok(value), notes, made: None },
            // A refusal in the CLI's own words, as `fwd` prints it after `Error: `.
            Err(e) => {
                Outcome { result: Err(format!("{e:#}")), notes, made: (!values.is_empty()).then(|| printed(values)) }
            }
        }
    }

    fn project_list(&self, args: &Map<String, Value>) -> Result<Value> {
        if let Some(key) = args.keys().next() {
            bail!("`{PROJECT_LIST}` takes no argument `{key}`");
        }
        let list: Vec<Value> = self
            .hosting
            .projects()
            .into_iter()
            .map(|(id, name, error)| json!({"id": id.to_string(), "name": name, "error": error}))
            .collect();
        Ok(Value::Array(list))
    }

    async fn whoami(&self, caller: &Caller, args: &Map<String, Value>) -> Result<Value> {
        let mut v = json!({
            "subject": caller.subject,
            "thread": caller.thread,
            "session": caller.session,
            "parent_thread": caller.parent,
            "main_thread": caller.main_thread(),
            "principal": null,
        });
        if let Some(key) = args.keys().find(|k| *k != tools::PROJECT) {
            bail!("`{WHOAMI}` takes no argument `{key}`; it takes project");
        }
        let Some(project) = args.get(tools::PROJECT) else { return Ok(v) };
        let (project, ctx) = self.project(project)?;
        v["project"] = json!({"id": project.id.to_string(), "name": project.name});
        match subject::principal_of(&ctx, &caller.subject).await? {
            Some(p) => v["principal"] = crate::principal_json(&ctx, &p).await?,
            None => {
                v["first_write_makes"] =
                    json!({"handle": self.free_handle(&ctx, caller).await?, "role": caller.default_role().to_string()});
                v["bind_instead"] = json!(format!(
                    "a lead from before automatic attachment, which worked by --as, is carried over to this thread by a person in a terminal: fwd admin bind <its name> --subject {} --project '{}'. Bind it before this thread's first call that writes or reads its own inbox or tasks, even one that is refused, which makes the thread a principal of its own; if that happened, retire that one first (fwd principal retire <its id>)",
                    caller.subject, project.name
                ));
            }
        }
        Ok(v)
    }

    /// The project a call names, by id or name, and a context on its node.
    fn project(&self, named: &Value) -> Result<(Project, Context)> {
        let named = named
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .context("`project` is a project's name or id, as project_list gives them")?;
        let hosted = self.hosting.projects();
        let by_id = named.parse::<Uuid>().ok().and_then(|id| hosted.iter().find(|(h, _, _)| *h == id));
        let found: Vec<&(Uuid, String, Option<String>)> = match by_id {
            Some(one) => vec![one],
            None => hosted.iter().filter(|(_, name, _)| name == named).collect(),
        };
        let (id, name, error) = match found.as_slice() {
            [one] => *one,
            [] => {
                let names: Vec<&str> = hosted.iter().map(|(_, n, _)| n.as_str()).collect();
                bail!(
                    "the daemon hosts no project named `{named}`; it hosts {}",
                    if names.is_empty() { "none".into() } else { names.join(", ") }
                )
            }
            several => bail!(
                "{} projects are named `{named}`: name one by its id ({})",
                several.len(),
                several.iter().map(|(id, _, _)| id.to_string()).collect::<Vec<_>>().join(", ")
            ),
        };
        if let Some(error) = error {
            bail!("the daemon could not open the project {name}: {error}");
        }
        self.hosting.open(*id).with_context(|| format!("the daemon does not have the project {name} open"))
    }

    /// Runs the CLI command `spec` stands for, for `caller`, as the CLI runs it after connecting
    /// (`crate::run_cli`): the command parsed by the CLI's parser, the principal the caller acts as
    /// found (and made when the command needs one), the CLI's refusals, the command, and its
    /// bookkeeping; its answer captured.
    /// What the command printed and said goes in `out`, whether it ran or failed.
    async fn command(
        &self,
        caller: &Caller,
        spec: &tools::Spec,
        args: &Map<String, Value>,
        out: &mut Captured,
    ) -> Result<()> {
        let (project, ctx) = self.project(args.get(tools::PROJECT).unwrap_or(&Value::Null))?;
        let argv = spec.argv(args).map_err(|e| anyhow!(e))?;
        // The CLI's grammar with no argument read from the environment: the daemon's FWD_AS,
        // FWD_PROJECT or FWD_SCOPE, inherited from whichever command started it, mean nothing to a
        // call, whose project and scope come from its arguments and whose caller from the transport.
        let matches = crate::Cli::command()
            .mut_args(|a| a.env(None::<&str>))
            .try_get_matches_from(&argv)
            .map_err(|e| anyhow!("{}", e.render().to_string().trim()))?;
        // Only an offered command runs: the one the tool stands for, checked on what was parsed.
        let words = crate::command_words(&matches).0;
        if words != spec.words {
            bail!("`{}` runs `fwd {}`, not `fwd {words}`", spec.name, spec.words);
        }
        let command = crate::Cli::from_arg_matches(&matches)?.command;
        crate::refuse_before_running(&command)?;
        let joining = crate::joins(&matches);
        let writes = joining || crate::WRITES_TOO.contains(&spec.words);
        let stamping = crate::stamps(joining, spec.words);
        let scope = match args.get(tools::SCOPE).filter(|v| !v.is_null()) {
            Some(s) => {
                let s = s.as_str().context(
                    "`scope` is a scope's id, or its first six characters or more, as the CLI's --scope takes",
                )?;
                // As the CLI reads `--scope` (`crate::caller`): an id, or its first characters.
                Some(ops::resolve_id(&ctx, s, &[ops::Kind::Scope]).await?)
            }
            None => None,
        };
        refuse_split(&ctx, caller).await?;
        // An agent that names itself, or names itself as another's successor, is made by its own
        // command with the name and role it gives, as the CLI makes it, and no principal is made
        // for it first; `principal succeed` from a thread with none is refused as the CLI refuses
        // it. Any other command that writes, or reads the caller's own inbox or tasks, acts as the
        // thread's principal, made on the first such call.
        let names_itself = matches!(
            command,
            Command::Principal { command: PrincipalCommand::Add { .. } | PrincipalCommand::Succeed { .. } }
        );
        let needs_actor = (writes && !names_itself) || crate::reads_about_caller(&command);
        let bound = subject::principal_of(&ctx, &caller.subject).await?;
        // Two first `principal_add`s of one thread arriving together make one principal: the
        // second runs once the first has bound it, and is refused as a second name.
        let _naming = match (&bound, names_itself) {
            (None, true) => Some(self.attaching.lock().await),
            _ => None,
        };
        let acting = match bound {
            Some(p) => Some(p.id()),
            None if needs_actor => Some(self.attach(&ctx, caller).await?),
            None => subject::principal_of(&ctx, &caller.subject).await?.map(|p| p.id()),
        };
        let me_handle = match acting {
            Some(p) => crate::handle_of(&ctx, p).await,
            None => String::new(),
        };
        let app = App {
            paths: self.hosting.paths().clone(),
            root: self.hosting.paths().storage_dir(&project),
            project: project.clone(),
            checkout: None,
            ctx: ctx.clone(),
            client: None,
            daemon_port: self.hosting.port(),
            out: Output::Captured(Default::default()),
            // A call that acts as no principal (a read about the graph alone, by a thread that has
            // not written yet) has no one to attribute anything to; the CLI would put the person
            // here, and the endpoint never acts as the person, so it puts an id no principal has,
            // which fails loudly wherever a command would read it. The CLI's refusals keep every
            // command that would from running ([`crate::refuse_by_identity`]).
            me: acting.unwrap_or_else(EntityId::random),
            me_handle,
            acting,
            subject: Some(caller.subject.clone()),
            typed_as: None,
            scope,
            scope_typed: scope.is_some(),
            several: None,
            format: Format::Json,
        };
        crate::refuse_by_identity(&app, &command, writes).await?;
        let stamped = crate::run_as_caller(&app, command, joining, stamping).await;
        *out = match app.out {
            Output::Captured(captured) => captured.into_inner().unwrap_or_else(|e| e.into_inner()),
            Output::Stdout => unreachable!("captured above"),
        };
        // The principal the command acted as, or the one it named.
        if let Some(principal) = match acting {
            Some(p) => Some(p),
            None => subject::principal_of(&ctx, &caller.subject).await.ok().flatten().map(|p| p.id()),
        } {
            self.seen.seen(project.id, principal, caller);
        }
        stamped?.context(crate::STAMP_FAILED)
    }

    /// The principal of the project bound to `caller`, made now when there is none: an agent with
    /// the first of the caller's names no principal has ([`Caller::handles`]) and its default role
    /// ([`Caller::default_role`]), as `fwd principal add` makes one (`ops::add_principal`), bound
    /// to the caller's subject. Made under [`Operations::attaching`], so that one thread's first
    /// calls arriving together make one principal.
    async fn attach(&self, ctx: &Context, caller: &Caller) -> Result<EntityId> {
        let _attaching = self.attaching.lock().await;
        if let Some(p) = subject::principal_of(ctx, &caller.subject).await? {
            return Ok(p.id());
        }
        let handle = self.free_handle(ctx, caller).await?;
        let id = ops::add_principal(ctx, &handle, Some(caller.default_role()), None, None, None, Some(&caller.subject))
            .await?;
        tracing::info!("attached {} as @{handle}", caller.subject);
        Ok(id)
    }

    /// The first of `caller`'s names no principal of the project has, or is named by.
    async fn free_handle(&self, ctx: &Context, caller: &Caller) -> Result<String> {
        for handle in caller.handles() {
            if fwd_core::naming::principal_named(ctx, &handle, None).await.ok().flatten().is_none() {
                return Ok(handle);
            }
        }
        bail!("every name fwd gives {} is taken in this project", subject::words(&caller.subject))
    }
}

/// The `help` tool: the CLI's `--help` for the command a tool stands for (`tools::long_help`).
fn help(args: &Map<String, Value>) -> Result<Value> {
    if let Some(key) = args.keys().find(|k| *k != "command") {
        bail!("`{HELP}` takes no argument `{key}`; it takes command");
    }
    let named = args.get("command").and_then(Value::as_str).context("`command` is a tool's name or its CLI words")?;
    Ok(json!({"help": tools::long_help(named).map_err(|e| anyhow!(e))?}))
}

/// The commands that read the caller's own inbox or tasks (`crate::reads_about_caller`), and so
/// make the caller's principal on a thread's first call: `task list` with `mine`, the rest always.
const CALLER_READS: &[&str] = &["inbox", "inbox pop", "prime", "task next", "task list"];

/// Refuses a call from a thread that a principal is bound to under another session than the one
/// this call carries: Codex keeps a thread's session across resumes in the build tested, and should
/// another build or path report another, the thread would otherwise get a second principal, its
/// inbox and its work split between two. A retired principal counts for nothing here, so a person
/// who retires the old one lets the thread be made anew; no command moves a principal to another
/// session.
async fn refuse_split(ctx: &Context, caller: &Caller) -> Result<()> {
    let thread = |s: &str| s.strip_prefix("codex:").and_then(|rest| rest.rsplit_once('/')).map(|(_, t)| t.to_string());
    for p in ops::principals(ctx).await? {
        // A retired principal does not split the thread: the person retired it.
        if !p.active().unwrap_or(false) {
            continue;
        }
        let Some(bound) = p.subject().ok().flatten() else { continue };
        if bound != caller.subject && thread(&bound).as_deref() == Some(caller.thread.as_str()) {
            bail!(
                "this thread, {}, is bound to @{} as {bound}, under another session than this call's ({}): fwd refuses rather than make the thread a second principal. No command moves a principal to another session; a person can retire @{} in a terminal (`fwd principal retire {}`), and the thread's next call that needs one then makes it anew under this session, its earlier work staying with @{}",
                caller.thread,
                p.handle()?,
                caller.session,
                p.handle()?,
                p.id().to_base64(),
                p.handle()?
            );
        }
    }
    Ok(())
}

/// Whether the command `words` writes, as the CLI counts writes.
fn writes(words: &str) -> bool {
    crate::JOINING.iter().any(|(line, w)| *line == words && *w != crate::Writes::No)
        || crate::WRITES_TOO.contains(&words)
}

/// Every tool, as `tools/list` gives it: the two of the endpoint's own, and one per offered
/// command (`tools::specs`).
pub fn catalog(specs: &[tools::Spec]) -> Vec<Tool> {
    let object = |v: Value| Arc::new(v.as_object().cloned().unwrap_or_default());
    let mut out = vec![
        Tool::new(
            PROJECT_LIST,
            "The projects the daemon hosts, by name and id (`fwd project list`). Every other tool but whoami and help takes one as `project`, and an optional `scope` (a scope's id or its first six characters or more: the CLI's --scope; none without it). Makes no principal.",
            object(json!({"type": "object", "properties": {}, "additionalProperties": false})),
        )
        .with_annotations(ToolAnnotations::new().read_only(true)),
        Tool::new(
            WHOAMI,
            "Who this thread is to fwd (`fwd whoami`): its subject and ids, and given a project the principal it acts as there, or the one its first call that writes or reads its own inbox or tasks would make, an implementer named after its thread unless principal_add names it first. Every call acts as the calling thread, from the ids the host attaches; no tool takes an actor. Makes no principal.",
            object(json!({
                "type": "object",
                "properties": {tools::PROJECT: {"type": "string", "description": "A project's name or id, as project_list gives them."}},
                "additionalProperties": false,
            })),
        )
        .with_annotations(ToolAnnotations::new().read_only(true)),
        Tool::new(
            HELP,
            "The CLI's full help for a tool's command (`fwd <command> --help`): what each tool's short description leaves out. Each argument there is the tool's property of the same name; --as, --format and file arguments are not taken.",
            object(json!({
                "type": "object",
                "properties": {"command": {"type": "string", "description": "A tool's name (task_create) or its CLI words (task create)"}},
                "required": ["command"],
            })),
        )
        .with_annotations(ToolAnnotations::new().read_only(true)),
    ];
    for spec in specs {
        // A read of the caller's own inbox or tasks makes the caller's principal on its first
        // call, so it is not read-only either. Only a read-only tool is annotated, which is all
        // the annotation says: a write keeps MCP's defaults (not read-only, possibly destructive),
        // claiming nothing it cannot keep.
        let writes = writes(spec.words) || CALLER_READS.contains(&spec.words);
        let tool = Tool::new(spec.name.clone(), spec.description.clone(), object(Value::Object(spec.schema.clone())));
        out.push(if writes { tool } else { tool.with_annotations(ToolAnnotations::new().read_only(true)) });
    }
    out
}

/// The endpoint as an MCP server: the transport half, which reads who calls from each request and
/// hands the call to the [`Operations`].
#[derive(Clone)]
pub struct Server {
    ops: Arc<Operations>,
}

impl Server {
    pub fn new(ops: Arc<Operations>) -> Server {
        Server { ops }
    }
}

impl ServerHandler for Server {
    /// The endpoint's name and build, and no `instructions`. Codex 0.159.2 shows the model the tools
    /// of every plugin's MCP servers, in name order, only within one budget of about 60,000 bytes
    /// that all installed plugins share, and charges each tool its definition plus its server's
    /// whole `instructions`: fwd's 648 characters, charged 74 times, hid all but 48 of its tools
    /// (/private/tmp/fwd-connections-active/catalog-limit-report.md). What they said is in the
    /// tools themselves: `project_list` says what `project` and `scope` are, `whoami` how a thread
    /// is attributed and named, and the plugin's skills the rest.
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("fwd", crate::BUILD).with_title("fwdcentaur"))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(self.ops.tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        if !self.ops.has(&request.name) {
            return Err(ErrorData::invalid_params(format!("no tool is named `{}`", request.name), None));
        }
        // The request's `_meta`, which rmcp hands the handler in the context; the request's own copy
        // where the context has none.
        let meta: Map<String, Value> = if context.meta.0 .0.is_empty() {
            request.meta.as_ref().map(|m| m.0 .0.clone()).unwrap_or_default()
        } else {
            context.meta.0 .0.clone()
        };
        let outcome = match Caller::from_codex_meta(&meta) {
            Ok(caller) => self.ops.call(&caller, &request.name, &request.arguments.unwrap_or_default()).await,
            Err(e) => Outcome::refused(format!("{e:#}")),
        };
        Ok(result_of(outcome).into())
    }
}

/// A call's outcome as MCP gives a tool's result: the command's JSON under `result`, or the
/// refusal under `error`, with what it said beside it under `notes`, as structured content and as
/// text for a client that reads only text. A refusal is a tool error the model reads, not a
/// protocol error.
pub fn result_of(outcome: Outcome) -> CallToolResult {
    let Outcome { result, notes, made } = outcome;
    let mut content = Vec::new();
    let (structured, failed) = match result {
        Ok(value) => {
            content.push(ContentBlock::text(serde_json::to_string_pretty(&value).unwrap_or_default()));
            (json!({"result": value, "notes": notes}), false)
        }
        Err(error) => {
            content.push(ContentBlock::text(error.clone()));
            let mut refused = json!({"error": error, "notes": notes});
            if let Some(made) = made {
                content.push(ContentBlock::text(serde_json::to_string_pretty(&made).unwrap_or_default()));
                refused["result"] = made;
            }
            (refused, true)
        }
    };
    for note in &notes {
        content.push(ContentBlock::text(note.clone()));
    }
    let mut result = if failed { CallToolResult::error(content) } else { CallToolResult::success(content) };
    result.structured_content = Some(structured);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The most bytes the catalog (`tools/list`, each tool's compact JSON summed) may take. Codex
    /// 0.159.2 shows the model plugin tools only within one budget of about 60,000 bytes shared by
    /// every installed plugin (catalog-limit-report.md; `Server::get_info`), so a compact catalog
    /// leaves room for the person's other plugins: at 73 KB fwd's alone overflowed it. Each
    /// description is the CLI's short help, abridged (`tools::short`), and the full help is one
    /// `help` call away. The installed-plugin harness's visible count is the acceptance check.
    const CATALOG_BUDGET: usize = 36_000;

    /// The catalog's size as a client receives it: each tool's compact JSON, summed.
    pub(crate) fn served_bytes(tools: &[Tool]) -> usize {
        tools.iter().map(|t| serde_json::to_string(t).expect("a tool serializes").len()).sum()
    }

    #[test]
    fn the_catalog_fits_the_hosts_budget_with_every_tool_in_it() {
        let tools = catalog(&tools::specs());
        let bytes = served_bytes(&tools);
        let mut sizes: Vec<(usize, String)> =
            tools.iter().map(|t| (serde_json::to_string(t).unwrap().len(), t.name.to_string())).collect();
        sizes.sort();
        assert!(
            bytes <= CATALOG_BUDGET,
            "the catalog is {bytes} bytes, over {CATALOG_BUDGET}; the largest: {:?}",
            &sizes[sizes.len() - 8..]
        );
        assert_eq!(tools.len(), tools::OFFERED.len() + 3, "every offered command, project_list, whoami and help");
        for name in ["task_create", "task_start", "task_submit", "reply", "whoami", "scope_list", "step_add", "unlink"]
        {
            assert!(tools.iter().any(|t| t.name == name), "{name} is in the catalog");
        }
    }

    /// What Codex 0.159.2 charges a plugin's tools against the budget all installed plugins share,
    /// about 60,000: each tool's definition plus its server's whole `initialize` instructions
    /// (catalog-limit-report.md). fwd sends no instructions, and its charge stays at most
    /// `CATALOG_BUDGET`, leaving the rest to the person's other plugins.
    #[test]
    fn what_codex_charges_the_plugins_tools_leaves_room_in_its_shared_budget() {
        let server = Server::new(Arc::new(Operations::new(Arc::new(NoHosting), Arc::new(NoWake))));
        let instructions = server.get_info().instructions.unwrap_or_default();
        assert!(instructions.is_empty(), "Codex charges the instructions once per tool: {instructions}");
        let tools = catalog(&tools::specs());
        let charged: usize = served_bytes(&tools) + tools.len() * instructions.len();
        assert!(charged <= CATALOG_BUDGET && CATALOG_BUDGET < 60_000, "{charged}");
    }

    /// A daemon with no project, for the tests that need an `Operations` and read none.
    struct NoHosting;
    impl Hosting for NoHosting {
        fn paths(&self) -> &Paths {
            unimplemented!("the test reads no path")
        }
        fn port(&self) -> u16 {
            0
        }
        fn projects(&self) -> Vec<(Uuid, String, Option<String>)> {
            Vec::new()
        }
        fn open(&self, _: Uuid) -> Option<(Project, Context)> {
            None
        }
    }

    #[test]
    fn the_help_tool_gives_the_clis_own_help_for_an_offered_command() {
        let help = |command: &str| super::help(&json!({"command": command}).as_object().cloned().unwrap());
        let by_name = help("reply").unwrap()["help"].as_str().unwrap().to_string();
        assert!(by_name.contains("Given a notification's id") && by_name.contains("settled"), "{by_name}");
        assert_eq!(help("task_create").unwrap(), help("task create").unwrap());
        assert!(help("task_create").unwrap()["help"].as_str().unwrap().contains("--done-when"));
        assert!(help("watch").is_err(), "no tool is the watch");
        assert!(help("daemon stop").is_err());
    }
}
