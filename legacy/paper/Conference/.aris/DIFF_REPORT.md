# DIFF REPORT — Conference vs System Design
# Date: 2026-07-14

## main.tex

--- paper/system-design/main.tex	2026-07-15 03:22:10.977893400 +0800
+++ paper/Conference/main.tex	2026-07-15 10:08:48.441283000 +0800
@@ -1,5 +1,5 @@
 % Pigs: Protocol-Native Phased Orchestration for LLM API Requests
-% System Design Paper
+% Full Conference Paper (English)
 
 \documentclass[10pt]{article}
 \usepackage[utf8]{inputenc}
@@ -9,6 +9,7 @@
 \usepackage{algorithm}
 \usepackage{algpseudocode}
 \usepackage{booktabs}
+\usepackage{multirow}
 \usepackage{graphicx}
 \usepackage{xcolor}
 \usepackage{tikz}
@@ -46,15 +47,18 @@
 \maketitle
 
 \begin{abstract}
-Large Language Model (LLM) agents typically interact with providers through stateless API calls, lacking built-in mechanisms for planning, execution verification, and failure recovery. While frameworks like ReAct and Reflexion add multi-stage reasoning within a single conversation, they rely on the LLM itself to manage control flow---an approach that is fragile, protocol-specific, and opaque to downstream tooling. We present \textbf{Pigs}, a phased orchestration middleware that wraps LLM API requests with a deterministic Pre$\rightarrow$Executor$\rightarrow$Post state machine using lightweight text control markers (\texttt{PIGEND}/\texttt{PIGFAIL}) for routing. Pigs preserves the complete protocol-native HTTP request---method, path, headers, and JSON body---across three API protocols (OpenAI Chat Completions, Anthropic Messages, OpenAI Responses), modifying only the model identifier, current user input, and phase transcript. Phase subrequests re-enter the local proxy via HTTP loopback, transparently inheriting channel selection, model mapping, body cleaning, and retry logic. External tool execution is supported through a bounded in-memory continuation store with TTL and capacity limits. We describe the architecture, key design decisions, and implementation in a 13-crate Rust workspace.
+Large Language Model (LLM) agents typically interact with providers through stateless API calls, lacking built-in mechanisms for planning, execution verification, and failure recovery. While frameworks like ReAct and Reflexion add multi-stage reasoning within a single conversation, they rely on the LLM itself to manage control flow---an approach that is fragile, protocol-specific, and opaque to downstream tooling. We present \textbf{Pigs}, a phased orchestration middleware that wraps LLM API requests with a deterministic Pre$\rightarrow$Executor$\rightarrow$Post state machine using lightweight text control markers (\texttt{PIGEND}/\texttt{PIGFAIL}) for routing. Pigs preserves the complete protocol-native HTTP request---method, path, headers, and JSON body---across three API protocols (OpenAI Chat Completions, Anthropic Messages, OpenAI Responses), modifying only the model identifier, current user input, and phase transcript. Phase subrequests re-enter the local proxy via HTTP loopback, transparently inheriting channel selection, model mapping, body cleaning, and retry logic. External tool execution is supported through a bounded in-memory continuation store with TTL and capacity limits. We evaluate Pigs through four experiments: an ablation study isolating each phase's contribution, a control marker effectiveness study, a three-protocol consistency evaluation, and a cost-benefit analysis. \textbf{Experimental results are pending and will be filled upon completion of the evaluation.}
 \end{abstract}
 
 \input{sections/introduction}
 \input{sections/background}
 \input{sections/design}
 \input{sections/implementation}
+\input{sections/experiments}
+\input{sections/results}
 \input{sections/related}
 \input{sections/discussion}
+\input{sections/conclusion}
 
 \bibliographystyle{plainnat}
 \bibliography{references}
(files differ)

## Section-by-section diffs

### background.tex — unchanged

### design.tex — unchanged

### discussion.tex (12 diff lines)

--- paper/system-design/sections/discussion.tex	2026-07-15 02:49:39.139973000 +0800
+++ paper/Conference/sections/discussion.tex	2026-07-15 10:26:53.610859300 +0800
@@ -19,17 +19,10 @@
 
 \subsection{Limitations}
 
-\textbf{Marker robustness.} The control-marker approach depends on the model's ability to follow instructions about when to output \texttt{PIGEND}/\texttt{PIGFAIL}. In practice, models sometimes output markers in inappropriate contexts (e.g., quoting the marker in prose). The marker detection algorithm mitigates this by checking only the last non-empty line and requiring a preceding reason, but it is not foolproof.
+\textbf{Marker robustness.} The control-marker approach depends on the model's ability to follow instructions about when to output \texttt{PIGEND}/\texttt{PIGFAIL}. In practice, models sometimes output markers in inappropriate contexts (e.g., quoting the marker in prose). The marker detection algorithm mitigates this by checking only the last non-empty line and requiring a preceding reason, but it is not foolproof. Empirical measurement of false positive and false negative rates is left for future evaluation.
 
 \textbf{Single-model assumption.} The current design uses the same model for all three phases. Multi-model orchestration (e.g., a cheaper model for Pre, a stronger model for Executor) is architecturally possible but not yet implemented.
 
 \textbf{No formal verification.} The state machine is tested with unit tests covering all transitions, but lacks formal verification (e.g., model checking) of properties like ``the system always terminates'' or ``no phase is skipped.''
 
 \textbf{Protocol coverage.} While three major protocols are supported, the system does not cover all provider-specific extensions (e.g., Google Gemini's API).
-
-\section{Conclusion}
-\label{sec:conclusion}
-
-We presented Pigs, a phased orchestration middleware for LLM API requests that combines a deterministic Pre$\rightarrow$Executor$\rightarrow$Post state machine with protocol-native request preservation, HTTP loopback transport, and bounded in-memory continuation. By elevating orchestration from a prompting technique to a middleware concern, Pigs separates deterministic control flow from stochastic LLM calls, making the orchestration transparent, debuggable, and protocol-agnostic. The system is implemented as a 13-crate Rust workspace and supports three major API protocols with real SSE streaming.
-
-Future work includes multi-model phase orchestration, formal verification of the state machine, and empirical evaluation on public agent benchmarks (SWE-bench, GAIA, AgentBench) to quantify the impact of phased orchestration on task completion quality.

### implementation.tex — unchanged

### introduction.tex (15 diff lines)

--- paper/system-design/sections/introduction.tex	2026-07-15 02:44:42.694855900 +0800
+++ paper/Conference/sections/introduction.tex	2026-07-15 10:27:25.244759800 +0800
@@ -15,15 +15,17 @@
     \item \textbf{Post} (review): the model independently reviews the executor's output against the goal. It may confirm completion (\texttt{PIGEND}), signal failure (\texttt{PIGFAIL}), or continue working (no marker).
 \end{enumerate}
 
-The state machine is driven by \emph{control markers}---short text tokens (\texttt{PIGEND}, \texttt{PIGFAIL}) that the model appends to its output. These markers are detected by the middleware, not the model's own reasoning; they serve as routing signals, not as content. Crucially, the middleware strips these markers from the visible output, so the client sees only the model's substantive text.
+The state machine is driven by \emph{control markers}---short text tokens (\texttt{PIGEND}, \texttt{PIGFAIL}) that the model appends to its output. These markers are detected by the middleware, not the model's own reasoning; they serve as routing signals, not as content. Crucially, the middleware strips these markers from the visible output, so the client sees only the model's substantive text. Unlike ReAct, where the model may skip planning or reflection, Pigs \emph{guarantees} that every non-trivial request passes through all three phases---the state machine cannot be bypassed by the model.
 
-Pigs makes four key contributions:
+Pigs makes six key contributions:
 
 \begin{enumerate}
     \item \textbf{Phased orchestration via text control markers}: a deterministic Pre$\rightarrow$Executor$\rightarrow$Post state machine that uses lightweight text markers for routing, without injecting orchestration-specific tools or system prompts into the model's context.
     \item \textbf{Protocol-native request preservation}: the complete HTTP request---method, path/query, headers, and JSON body---is preserved across three API protocols, with only the model identifier, current user input, and phase transcript modified. Unknown fields, media blocks, and provider extensions pass through untouched.
     \item \textbf{HTTP loopback transport}: phase subrequests re-enter the local proxy via HTTP, transparently inheriting channel selection, model mapping, body cleaning, thinking-effort injection, and retry logic---without requiring a separate in-process dispatch path.
     \item \textbf{Bounded in-memory continuation}: external tool execution is supported through a continuation store with TTL, capacity limits, and tombstone-based error reporting, never persisting authentication headers to disk.
+    \item \textbf{Ablation study} isolating the contribution of each phase (Pre, Executor, Post) to overall task performance on public benchmarks (SWE-bench, GAIA).
+    \item \textbf{Control marker effectiveness study} showing whether PIGFAIL-based failure recovery improves outcomes, plus a \textbf{three-protocol consistency evaluation} and \textbf{cost-benefit analysis} quantifying the token, latency, and API call overhead of orchestration.
 \end{enumerate}
 
-The remainder of this paper describes the architecture (\Cref{sec:design}), implementation (\Cref{sec:impl}), related work (\Cref{sec:related}), and design discussion (\Cref{sec:discussion}).
+The remainder of this paper describes the architecture (\Cref{sec:design}), implementation (\Cref{sec:impl}), experimental evaluation (\Cref{sec:experiments}), related work (\Cref{sec:related}), and design discussion (\Cref{sec:discussion}).

### related.tex (12 diff lines)

--- paper/system-design/sections/related.tex	2026-07-15 03:28:29.518114900 +0800
+++ paper/Conference/sections/related.tex	2026-07-15 10:21:36.977002200 +0800
@@ -21,11 +21,11 @@
 
 \subsection{LLM Middleware and Proxies}
 
-\textbf{Towards a Middleware for LLMs}~\citep{dzanic2024middleware} is a vision paper proposing middleware for enterprise LLM deployment, but provides no implementation. Pigs is an implemented system that realizes this vision with a specific orchestration model.
+A position paper~\citep{dzanic2024middleware} proposes middleware for enterprise LLM deployment, but provides no implementation. Pigs is an implemented system that realizes this vision with a specific orchestration model.
 
-\textbf{TokenMizer}~\citep{liu2026tokenmizer} is a transparent proxy with session state management via a typed knowledge graph. While it adds state to the proxy layer, it does not impose phase structure or control markers.
+\textbf{TokenMizer}~\citep{liu2026tokenmizer} adds session state management to the proxy layer. While it introduces state at the proxy level, it does not impose phase structure or control markers.
 
-\textbf{STREAM}~\citep{stream2026} implements multi-tier LLM inference routing (local$\rightarrow$HPC$\rightarrow$cloud) with a complexity judge. Pigs' routing is phase-based (Pre$\rightarrow$Executor$\rightarrow$Post), not tier-based.
+\textbf{STREAM}~\citep{stream2026} is a multi-tier LLM inference routing system (local$\rightarrow$HPC$\rightarrow$cloud) with a complexity judge. Pigs' routing is phase-based (Pre$\rightarrow$Executor$\rightarrow$Post), not tier-based.
 
 \textbf{Dyserve}~\citep{dyserve2027} is a workflow-aware serving layer that reroutes on tool-call failures. Pigs' \texttt{PIGFAIL} marker provides a similar failure-recovery mechanism, but at the phase level rather than the tool-call level.
 


## New files (not in system-design)

### conclusion.tex (NEW)

\section{Conclusion}
\label{sec:conclusion}

We presented Pigs, a phased orchestration middleware for LLM API requests that combines a deterministic Pre$\rightarrow$Executor$\rightarrow$Post state machine with protocol-native request preservation, HTTP loopback transport, and bounded in-memory continuation. By elevating orchestration from a prompting technique to a middleware concern, Pigs separates deterministic control flow from stochastic LLM calls, making the orchestration transparent, debuggable, and protocol-agnostic. The system is implemented as a 13-crate Rust workspace and supports three major API protocols with real SSE streaming.

We designed an experimental evaluation framework comprising four experiments: an ablation study isolating each phase's contribution, a control marker effectiveness study evaluating PIGFAIL-based failure recovery, a three-protocol consistency evaluation, and a cost-benefit analysis. All experiments use public benchmarks (SWE-bench, GAIA, AgentBoard) and require only API access---no GPU resources. \textbf{Experimental results are pending and will be reported upon completion of the evaluation.}

Future work includes: (1) multi-model phase orchestration (different models for different phases), (2) formal verification of the state machine, (3) extended benchmarks (ToolBench, BFCL), (4) security analysis against prompt injection attacks~\citep{greshake2023promptinjection}, and (5) marker robustness improvements using fine-tuned detection models.

### experiments.tex (NEW)

\section{Experiments}
\label{sec:experiments}

To evaluate whether phased orchestration improves task completion quality, we design four experiments that collectively address the concerns raised in prior review: (1) an ablation study isolating each phase's contribution, (2) a control marker effectiveness study, (3) a three-protocol consistency evaluation, and (4) a cost-benefit analysis. All experiments are conducted through the Pigs HTTP API, requiring only API access---no GPU resources.

\subsection{Experimental Setup}

\textbf{System configuration.} All experiments use the Pigs API server with default orchestration limits (\texttt{max\_post\_iterations=3}, \texttt{max\_pre\_replans=2}), continuation TTL of 30 minutes, capacity of 256, and Chinese (zh) phase prompts. Thinking effort is set to xhigh for OpenAI and max for Anthropic.

\textbf{Benchmarks.} We use three public benchmarks:

\begin{center}
\begin{tabular}{lllr}
\toprule
Benchmark & Domain & Tasks & Public \\
\midrule
SWE-bench Lite~\citep{jimenez2024swebench} & Software engineering & 300 & \checkmark \\
GAIA~\citep{mialon2023gaia} & General AI assistant & 300 & \checkmark \\
AgentBoard~\citep{ma2024agentboard} & Multi-turn agent & 9 tasks & \checkmark \\
\bottomrule
\end{tabular}
\end{center}

\textbf{Ablation conditions.} For Experiment 1, we define four conditions:
\begin{itemize}[nosep]
    \item \textbf{Condition A (Baseline)}: Direct API call without orchestration (non-\texttt{-pig} model, passthrough).
    \item \textbf{Condition B (Full)}: Complete Pre$\rightarrow$Executor$\rightarrow$Post orchestration (\texttt{-pig} model).
    \item \textbf{Condition C (Pre-only)}: Only the Pre phase (planning without execution verification).
    \item \textbf{Condition D (Executor+Post)}: Skip Pre, directly execute and review.
\end{itemize}

\textbf{Metrics.} We measure: (1) task completion rate (benchmark-specific), (2) average LLM API calls per task, (3) total input + output tokens per task, (4) wall-clock latency, and (5) error recovery rate (percentage of tasks where PIGFAIL triggered a successful replan).

\subsection{Experiment 1: Ablation Study}

\textbf{Setup.} We evaluate four conditions (A: baseline, B: full orchestration, C: Pre-only, D: Executor+Post) on SWE-bench Lite (300 tasks) and GAIA (300 tasks). Each task is sent as a \texttt{-pig} model request (Condition B/C/D) or a passthrough request (Condition A) to the Pigs API.

\textbf{Procedure.} For each task:
\begin{enumerate}[nosep]
    \item Send the task prompt to the Pigs API endpoint.
    \item If tool calls are returned, execute them locally and send results back.
    \item Repeat until the API returns a final response (no tool calls).
    \item Evaluate the final response against the benchmark's ground truth.
\end{enumerate}

\textbf{Hypothesis.} Full orchestration (B) outperforms baseline (A) on task completion rate, with the Pre phase contributing planning and the Post phase contributing verification.

\subsection{Experiment 2: Control Marker Effectiveness}

\textbf{Setup.} We compare two configurations on GAIA: (A) with \texttt{PIGFAIL} enabled (failure triggers replan), (B) with \texttt{PIGFAIL} disabled (failure treated as completion).

\textbf{Procedure.} We count: (1) tasks where PIGFAIL was emitted, (2) tasks where replan led to successful completion, (3) tasks that failed despite replan.

\textbf{Hypothesis.} PIGFAIL-based replan improves outcomes on tasks where the initial approach was flawed.

\subsection{Experiment 3: Three-Protocol Consistency}

\textbf{Setup.} We send the same set of 100 tasks through three API protocols: OpenAI Chat (\texttt{/chat/completions}), Anthropic Messages (\texttt{/v1/messages}), and OpenAI Responses (\texttt{/responses}).

\textbf{Procedure.} For each task and protocol, we record: (1) whether the task completed, (2) the visible text output, (3) any tool calls made. We measure consistency as the percentage of tasks where all three protocols agree on completion status.

\textbf{Hypothesis.} The protocol-native request preservation ensures consistent behavior across all three protocols.

\subsection{Experiment 4: Cost-Benefit Analysis}

\textbf{Setup.} On SWE-bench Lite, we measure per-task: (1) total input tokens, (2) total output tokens, (3) number of API calls, (4) wall-clock latency, (5) task completion.

\textbf{Procedure.} We compare Condition A (baseline) vs.\ Condition B (full orchestration) and compute the overhead ratio for each metric.

\textbf{Hypothesis.} The token overhead of orchestration (3 phases vs.\ 1) is justified by improved completion rate.

### results.tex (NEW)

\section{Results}
\label{sec:results}

\textbf{[All experimental results are pending. The tables below will be filled upon completion of the evaluation.]}

\subsection{Experiment 1: Ablation Results}

\begin{center}
\begin{tabular}{llcccc}
\toprule
Benchmark & Condition & Completion & Avg.\ Turns & Tokens & Latency \\
 & & Rate (\%) & & (k) & (s) \\
\midrule
\multirow{4}{*}{SWE-bench Lite} & A (Baseline) & --- & --- & --- & --- \\
 & B (Full) & --- & --- & --- & --- \\
 & C (Pre-only) & --- & --- & --- & --- \\
 & D (Exec+Post) & --- & --- & --- & --- \\
\midrule
\multirow{4}{*}{GAIA} & A (Baseline) & --- & --- & --- & --- \\
 & B (Full) & --- & --- & --- & --- \\
 & C (Pre-only) & --- & --- & --- & --- \\
 & D (Exec+Post) & --- & --- & --- & --- \\
\bottomrule
\end{tabular}
\end{center}

\subsection{Experiment 2: Control Marker Results}

\begin{center}
\begin{tabular}{lccc}
\toprule
Configuration & PIGFAIL emitted & Replan succeeded & Overall completion \\
 & (\%) & (\%) & (\%) \\
\midrule
A (With PIGFAIL) & --- & --- & --- \\
B (Without PIGFAIL) & N/A & N/A & --- \\
\bottomrule
\end{tabular}
\end{center}

\subsection{Experiment 3: Protocol Consistency}

\begin{center}
\begin{tabular}{lcccc}
\toprule
Protocol & Completion (\%) & Tool calls & Text agreement & Consistency \\
\midrule
OpenAI Chat & --- & --- & --- & --- \\
Anthropic Messages & --- & --- & --- & --- \\
OpenAI Responses & --- & --- & --- & --- \\
\bottomrule
\end{tabular}
\end{center}

\subsection{Experiment 4: Cost-Benefit}

\begin{center}
\begin{tabular}{lcccc}
\toprule
Condition & Input tokens & Output tokens & API calls & Completion \\
 & (k) & (k) & & (\%) \\
\midrule
A (Baseline) & --- & --- & --- & --- \\
B (Full) & --- & --- & --- & --- \\
Overhead & ---$\times$ & ---$\times$ & ---$\times$ & ---pp \\
\bottomrule
\end{tabular}
\end{center}

