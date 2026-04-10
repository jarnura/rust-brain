#!/bin/bash
# CLASS A-E Agent Routing Test Script
# Tests the orchestrator dispatch logic without requiring cargo/rust toolchain
#
# Usage: ./tests/integration/test_class_routing.sh [CLASS]
#   CLASS: A, B, C, D, E, or "all" (default: all)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
API_BASE="${API_BASE:-http://localhost:8088}"
CLASS="${1:-all}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[PASS]${NC} $1"; }
log_fail() { echo -e "${RED}[FAIL]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }

# Check prerequisites
check_prereqs() {
    log_info "Checking prerequisites..."
    
    if ! curl -sf "${API_BASE}/health" > /dev/null 2>&1; then
        log_fail "API not responding at ${API_BASE}"
        log_info "Make sure the docker-compose stack is running: bash scripts/start.sh"
        exit 1
    fi
    log_success "API is healthy"
    
    # Check MCP
    if ! curl -sf "http://localhost:3001/sse" > /dev/null 2>&1; then
        log_warn "MCP-SSE not responding at localhost:3001"
    else
        log_success "MCP-SSE is responding"
    fi
}

# Create test workspace
create_workspace() {
    local name="test-class-${CLASS}-$(date +%s)"
    log_info "Creating workspace: $name"
    
    local response
    response=$(curl -sf "${API_BASE}/workspaces" \
        -X POST \
        -H "Content-Type: application/json" \
        -d "{\"repo_url\":\"https://github.com/jarnura/rust-brain.git\",\"name\":\"$name\",\"branch\":\"main\"}" 2>/dev/null) || {
        log_fail "Failed to create workspace"
        return 1
    }
    
    local workspace_id
    workspace_id=$(echo "$response" | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])" 2>/dev/null)
    
    if [[ -z "$workspace_id" ]]; then
        log_fail "Could not extract workspace ID from response"
        return 1
    fi
    
    echo "$workspace_id"
}

# Wait for workspace to be ready
wait_for_workspace() {
    local workspace_id="$1"
    local max_attempts=30
    local attempt=0
    
    log_info "Waiting for workspace $workspace_id to be ready..."
    
    while [[ $attempt -lt $max_attempts ]]; do
        local status
        status=$(curl -sf "${API_BASE}/workspaces/${workspace_id}" 2>/dev/null | \
            python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('status','unknown'))" 2>/dev/null || echo "error")
        
        if [[ "$status" == "ready" ]]; then
            log_success "Workspace is ready"
            return 0
        fi
        
        attempt=$((attempt + 1))
        sleep 2
    done
    
    log_fail "Workspace did not become ready within 60s"
    return 1
}

# Execute a query and return execution ID
execute_query() {
    local workspace_id="$1"
    local prompt="$2"
    
    log_info "Executing: $prompt"
    
    local response
    response=$(curl -sf "${API_BASE}/workspaces/${workspace_id}/execute" \
        -X POST \
        -H "Content-Type: application/json" \
        -d "{\"prompt\":\"$prompt\"}" 2>/dev/null) || {
        log_fail "Failed to start execution"
        return 1
    }
    
    local execution_id
    execution_id=$(echo "$response" | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])" 2>/dev/null)
    local status
    status=$(echo "$response" | python3 -c "import sys,json; print(json.load(sys.stdin)['status'])" 2>/dev/null)
    
    if [[ "$status" != "pending" ]]; then
        log_warn "Execution status is '$status', expected 'pending'"
    fi
    
    echo "$execution_id"
}

# Poll execution until terminal state
poll_execution() {
    local execution_id="$1"
    local max_wait="${2:-300}"  # Default 5 minutes
    local poll_interval=5
    local elapsed=0
    
    log_info "Polling execution $execution_id (max ${max_wait}s)..."
    
    while [[ $elapsed -lt $max_wait ]]; do
        local response
        response=$(curl -sf "${API_BASE}/executions/${execution_id}" 2>/dev/null) || {
            log_warn "Failed to poll execution"
            sleep $poll_interval
            elapsed=$((elapsed + poll_interval))
            continue
        }
        
        local status
        status=$(echo "$response" | python3 -c "import sys,json; print(json.load(sys.stdin).get('status','unknown'))" 2>/dev/null)
        local agent_phase
        agent_phase=$(echo "$response" | python3 -c "import sys,json; print(json.load(sys.stdin).get('agent_phase','none'))" 2>/dev/null)
        
        if [[ "$status" != "$last_status" ]] || [[ "$agent_phase" != "$last_phase" ]]; then
            log_info "Status: $status | Agent phase: $agent_phase | Elapsed: ${elapsed}s"
            last_status="$status"
            last_phase="$agent_phase"
        fi
        
        case "$status" in
            completed|failed|timeout|aborted)
                log_success "Execution reached terminal state: $status"
                return 0
                ;;
        esac
        
        sleep $poll_interval
        elapsed=$((elapsed + poll_interval))
    done
    
    log_warn "Execution did not complete within ${max_wait}s"
    return 1
}

# Get and analyze agent events
analyze_events() {
    local execution_id="$1"
    
    log_info "Analyzing agent events for $execution_id..."
    
    local events
    events=$(curl -sf "${API_BASE}/executions/${execution_id}/events" 2>/dev/null) || {
        log_fail "Failed to fetch events"
        return 1
    }
    
    # Count events by type
    local event_counts
    event_counts=$(echo "$events" | python3 << 'PYEOF'
import sys, json
events = json.load(sys.stdin)
counts = {}
for e in events:
    t = e.get('event_type', 'unknown')
    counts[t] = counts.get(t, 0) + 1
for t, c in sorted(counts.items()):
    print(f"  {t}: {c}")
PYEOF
)
    
    echo "$event_counts"
    
    # Extract agent_dispatch events
    local dispatch_agents
    dispatch_agents=$(echo "$events" | python3 << 'PYEOF'
import sys, json
events = json.load(sys.stdin)
agents = []
for e in events:
    if e.get('event_type') == 'agent_dispatch':
        agent = e.get('content', {}).get('agent', 'unknown')
        agents.append(agent)
if agents:
    print("Agents dispatched: " + " → ".join(agents))
else:
    print("No agent_dispatch events found")
PYEOF
)
    
    if [[ "$dispatch_agents" == "No agent_dispatch events found" ]]; then
        log_warn "$dispatch_agents"
        return 1
    else
        log_success "$dispatch_agents"
    fi
}

# Run CLASS A test
test_class_a() {
    log_info "========== CLASS A TEST =========="
    log_info "Query: 'What does PipelineRunner do?'"
    log_info "Expected: orchestrator → explorer"
    
    local workspace_id
    workspace_id=$(create_workspace) || return 1
    wait_for_workspace "$workspace_id" || return 1
    
    local execution_id
    execution_id=$(execute_query "$workspace_id" "What does PipelineRunner do?") || return 1
    log_success "Started execution: $execution_id"
    
    poll_execution "$execution_id" 180 || true  # Don't fail on timeout, still analyze
    analyze_events "$execution_id"
    
    # Check MCP logs for tool calls
    log_info "Recent MCP tool calls:"
    docker logs rustbrain-mcp-sse --since 3m 2>&1 | grep "Tool call:" | tail -5 || log_warn "No MCP calls found"
}

# Run CLASS B test
test_class_b() {
    log_info "========== CLASS B TEST =========="
    log_info "Query: 'How should I add retry logic to the pipeline?'"
    log_info "Expected: orchestrator → explorer → planner"
    
    local workspace_id
    workspace_id=$(create_workspace) || return 1
    wait_for_workspace "$workspace_id" || return 1
    
    local execution_id
    execution_id=$(execute_query "$workspace_id" "How should I add retry logic to the pipeline?") || return 1
    log_success "Started execution: $execution_id"
    
    poll_execution "$execution_id" 240 || true
    analyze_events "$execution_id"
}

# Run CLASS C test
test_class_c() {
    log_info "========== CLASS C TEST =========="
    log_info "Query: 'Add a doc comment to the run_execution function'"
    log_info "Expected: orchestrator → explorer → planner → developer → reviewer"
    
    local workspace_id
    workspace_id=$(create_workspace) || return 1
    wait_for_workspace "$workspace_id" || return 1
    
    local execution_id
    execution_id=$(execute_query "$workspace_id" "Add a doc comment to the run_execution function showing it is the main entry point") || return 1
    log_success "Started execution: $execution_id"
    
    poll_execution "$execution_id" 300 || true
    analyze_events "$execution_id"
}

# Run CLASS E test (simpler than D)
test_class_e() {
    log_info "========== CLASS E TEST =========="
    log_info "Query: 'Document the embedding pipeline'"
    log_info "Expected: orchestrator → documentation"
    
    local workspace_id
    workspace_id=$(create_workspace) || return 1
    wait_for_workspace "$workspace_id" || return 1
    
    local execution_id
    execution_id=$(execute_query "$workspace_id" "Document the embedding pipeline architecture") || return 1
    log_success "Started execution: $execution_id"
    
    poll_execution "$execution_id" 240 || true
    analyze_events "$execution_id"
}

# Main
echo "========================================"
echo "CLASS A-E Agent Routing Test"
echo "API: ${API_BASE}"
echo "========================================"

check_prereqs

case "$CLASS" in
    A|a)
        test_class_a
        ;;
    B|b)
        test_class_b
        ;;
    C|c)
        test_class_c
        ;;
    D|d)
        log_warn "CLASS D (full pipeline) takes 10+ minutes, skipping for quick tests"
        ;;
    E|e)
        test_class_e
        ;;
    all|ALL)
        test_class_a
        echo ""
        test_class_b
        echo ""
        test_class_e
        log_info "CLASS C and D skipped in 'all' mode (too slow)"
        log_info "Run individually: $0 C"
        ;;
    *)
        echo "Usage: $0 [CLASS]"
        echo "  CLASS: A, B, C, D, E, or all"
        exit 1
        ;;
esac

echo ""
log_info "Test complete. Check logs for full details:"
log_info "  docker logs rustbrain-api --since 10m"
log_info "  docker logs rustbrain-mcp-sse --since 10m"
