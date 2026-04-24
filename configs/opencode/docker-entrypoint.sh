#!/bin/bash
# =============================================================================
# OpenCode Docker Entrypoint
# =============================================================================
# Implements a git-based write workflow for the developer agent:
# 1. Clones target repo into a writable location
# 2. Configures git for commits
# 3. Creates a feature branch
# 4. Starts OpenCode server
# =============================================================================

set -e

# Configuration from environment
TARGET_REPO_URL="${TARGET_REPO_URL:-}"
TARGET_REPO_PATH="${TARGET_REPO_PATH:-/workspace/target-repo}"
WORK_DIR="${OPENCODE_WORK_DIR:-/workspace/target-repo-work}"
GIT_USER_NAME="${GIT_USER_NAME:-OpenCode Developer}"
GIT_USER_EMAIL="${GIT_USER_EMAIL:-opencode@rustbrain.local}"
FEATURE_BRANCH_PREFIX="${FEATURE_BRANCH_PREFIX:-opencode}"

# Colors for logging
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# =============================================================================
# Clone or Copy Target Repository
# =============================================================================
setup_work_directory() {
    log_info "Setting up work directory: $WORK_DIR"
    
    if [ -d "$WORK_DIR/.git" ]; then
        log_info "Work directory already initialized (persistent volume detected). Skipping clone/copy."
        cd "$WORK_DIR"
        
        if ! git rev-parse HEAD >/dev/null 2>&1; then
            log_info "No commits found. Creating initial commit..."
            git add -A
            git commit -m "Initial commit from $TARGET_REPO_PATH"
        fi
        
        return 0
    fi
    
    # If TARGET_REPO_URL is provided, clone from URL
    if [ -n "$TARGET_REPO_URL" ]; then
        log_info "Cloning from URL: $TARGET_REPO_URL"
        
        # Use GH_TOKEN for private repos if available
        if [ -n "$GH_TOKEN" ]; then
            # Inject token into URL for authentication
            CLONE_URL=$(echo "$TARGET_REPO_URL" | sed "s|https://|https://x-access-token:${GH_TOKEN}@|")
            git clone --depth 1 "$CLONE_URL" "$WORK_DIR"
        else
            git clone --depth 1 "$TARGET_REPO_URL" "$WORK_DIR"
        fi
        
    # Otherwise, copy from the mounted read-only directory
    elif [ -d "$TARGET_REPO_PATH" ] && [ "$(ls -A $TARGET_REPO_PATH 2>/dev/null)" ]; then
        log_info "Copying from mounted directory: $TARGET_REPO_PATH"
        
        # Create work directory
        mkdir -p "$WORK_DIR"
        
        # Copy all files including hidden ones (but excluding .git if it exists)
        # We'll re-init git to have a clean history
        rsync -a --exclude='.git' --exclude='target' --exclude='node_modules' "$TARGET_REPO_PATH/" "$WORK_DIR/"
        
        # Initialize git repo
        cd "$WORK_DIR"
        git init
        git add -A
        git commit -m "Initial commit from $TARGET_REPO_PATH"
        
    else
        log_error "No target repository available. Set TARGET_REPO_URL or mount a repo at $TARGET_REPO_PATH"
        exit 1
    fi
    
    cd "$WORK_DIR"
}

# =============================================================================
# Configure Git
# =============================================================================
configure_git() {
    log_info "Configuring git user: $GIT_USER_NAME <$GIT_USER_EMAIL>"

    git config --global user.name "$GIT_USER_NAME"
    git config --global user.email "$GIT_USER_EMAIL"

    # Configure git to handle line endings
    git config --global core.autocrlf input

    # Set default branch name
    git config --global init.defaultBranch main

    # Configure credential helper for HTTPS pushes (if GH_TOKEN is available)
    if [ -n "$GH_TOKEN" ]; then
        git config --global credential.helper store
        log_info "Git credentials configured for HTTPS push"
    fi
}

# =============================================================================
# Create Feature Branch
# =============================================================================
create_feature_branch() {
    CURRENT_BRANCH=$(git branch --show-current 2>/dev/null || echo "")
    
    if [ -n "$CURRENT_BRANCH" ] && echo "$CURRENT_BRANCH" | grep -q "^${FEATURE_BRANCH_PREFIX}/"; then
        log_info "Already on feature branch: $CURRENT_BRANCH (skipping branch creation)"
        BRANCH_NAME="$CURRENT_BRANCH"
    else
        TIMESTAMP=$(date +%Y%m%d-%H%M%S)
        BRANCH_NAME="${FEATURE_BRANCH_PREFIX}/changes-${TIMESTAMP}"
        
        log_info "Creating feature branch: $BRANCH_NAME"
        git checkout -b "$BRANCH_NAME"
    fi
    
    echo "$BRANCH_NAME" > /tmp/opencode-branch.txt
    
    log_info "Feature branch ready: $BRANCH_NAME"
    log_info "Developer agent can now write to: $WORK_DIR"
}

# =============================================================================
# Validate API Keys
# =============================================================================
validate_api_keys() {
    local VALID=true

    # LITELLM_API_KEY validation
    if [ -z "${LITELLM_API_KEY:-}" ]; then
        log_warn "LITELLM_API_KEY is not set — OpenCode will not be able to connect to LiteLLM proxy"
        VALID=false
    elif [ "$LITELLM_API_KEY" = "your-api-key-here" ]; then
        log_warn "LITELLM_API_KEY is still the placeholder value — set a real key in .env"
        VALID=false
    elif echo "$LITELLM_API_KEY" | grep -qP '^\s+$'; then
        log_warn "LITELLM_API_KEY is whitespace-only — set a real key in .env"
        VALID=false
    elif [ ${#LITELLM_API_KEY} -lt 8 ]; then
        log_warn "LITELLM_API_KEY appears too short (less than 8 chars) — expected a Bearer token or API key"
        VALID=false
    elif ! echo "$LITELLM_API_KEY" | grep -qP '^[A-Za-z0-9_\-.]+$'; then
        log_warn "LITELLM_API_KEY contains unexpected characters — expected alphanumeric, dashes, underscores, or dots"
        VALID=false
    fi

    if [ "$VALID" = true ]; then
        log_info "LITELLM_API_KEY format validated"
    else
        log_warn "OpenCode AI features (chat, model routing) will not work without a valid LITELLM_API_KEY"
    fi
}

# =============================================================================
# Verify OpenCode Config
# =============================================================================
verify_opencode_config() {
    local template="/home/opencode/.config/opencode/opencode.json.template"
    local config="/home/opencode/.config/opencode/opencode.json"

    # Retry up to 5 times — bind mounts may not be ready at container start
    local attempts=0
    while [ $attempts -lt 5 ] && [ ! -f "$template" ] && [ ! -f "$config" ]; do
        attempts=$((attempts + 1))
        log_warn "Config not found yet, waiting for bind mount (attempt $attempts/5)..."
        sleep 1
    done

    if [ -f "$template" ]; then
        log_info "Generating OpenCode config from template (substituting env vars)"
        sed \
            -e "s|\${LITELLM_API_KEY}|${LITELLM_API_KEY:-}|g" \
            -e "s|\${MCP_SSE_URL}|${MCP_SSE_URL:-http://mcp-sse:3001/sse}|g" \
            -e "s|\${LITELLM_BASE_URL}|${LITELLM_BASE_URL:-https://grid.ai.juspay.net}|g" \
            "$template" > "$config"
        log_info "OpenCode config written to $config"
    elif [ -f "$config" ]; then
        log_info "OpenCode config found at $config"
    else
        log_warn "OpenCode config not found after retries - using defaults"
    fi
}

# =============================================================================
# Main Entry Point
# =============================================================================
main() {
    log_info "=========================================="
    log_info "OpenCode Developer Agent Startup"
    log_info "=========================================="
    log_info "Target repo path: $TARGET_REPO_PATH"
    log_info "Target repo URL:  ${TARGET_REPO_URL:-<not set>}"
    log_info "Work directory:   $WORK_DIR"
    log_info "Git user:         $GIT_USER_NAME"
    log_info "Git email:        $GIT_USER_EMAIL"
    log_info "=========================================="
    
    # Step 1: Configure git FIRST so commits work during setup
    configure_git
    
    # Step 2: Set up the work directory (may commit, needs git user configured)
    setup_work_directory
    
    # Step 3: Create feature branch
    create_feature_branch
    
    # Step 4: Verify config
    verify_opencode_config
    
    # Step 5: Validate API keys
    validate_api_keys
    
    log_info "=========================================="
    log_info "Starting OpenCode server..."
    log_info "=========================================="
    
    # Export the work directory for the server
    export OPENCODE_WORK_DIR="$WORK_DIR"
    
    # Start OpenCode server
    # Pass all arguments to opencode
    exec opencode serve --port 4096 --hostname 0.0.0.0 "$@"
}

# Run main with all arguments
main "$@"
