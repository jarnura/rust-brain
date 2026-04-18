#!/usr/bin/env bash
#
# check-neo4j-isolation.sh
#
# CI script to enforce WorkspaceGraphClient usage in API handlers.
# Detects direct Neo4j access that bypasses workspace scoping.
#
# Usage: ./scripts/check-neo4j-isolation.sh
#
# Rules:
# - Handlers should use WorkspaceGraphClient for Neo4j access
# - Exemptions allowed via # RUSA-194-EXEMPT: <reason> comment on the same line or line above
# - health.rs is exempt for system-level health checks
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

HANDLERS_DIR="$PROJECT_ROOT/services/api/src/handlers"
EXEMPTION_MARKER="RUSA-194-EXEMPT"
EXIT_CODE=0

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

if [[ ! -d "$HANDLERS_DIR" ]]; then
    echo -e "${RED}ERROR:${NC} Handlers directory not found: $HANDLERS_DIR"
    exit 1
fi

echo "Checking Neo4j isolation in API handlers..."
echo "Directory: $HANDLERS_DIR"
echo ""

# Temporary file to collect violations
VIOLATIONS_FILE=$(mktemp)
trap "rm -f $VIOLATIONS_FILE" EXIT

# Check for execute_neo4j_query calls
echo "Scanning for execute_neo4j_query usage..."
while IFS=: read -r filepath lineno line; do
    filename=$(basename "$filepath")

    trimmed_line=$(echo "$line" | sed 's/^[[:space:]]*//')
    if [[ "$trimmed_line" =~ ^// ]]; then
        continue
    fi

    prev_lineno=$((lineno - 1))
    prev_line=""
    if [[ $prev_lineno -gt 0 ]]; then
        prev_line=$(sed -n "${prev_lineno}p" "$filepath" 2>/dev/null || true)
    fi

    if [[ "$line" =~ $EXEMPTION_MARKER ]] || [[ "$prev_line" =~ $EXEMPTION_MARKER ]]; then
        echo -e "  ${GREEN}✓${NC} $filename:$lineno (exempted)"
    else
        echo -e "  ${RED}✗${NC} $filename:$lineno - unexempted execute_neo4j_query usage"
        echo "$filepath:$lineno: $line" >> "$VIOLATIONS_FILE"
        EXIT_CODE=1
    fi
done < <(grep -rn "execute_neo4j_query" "$HANDLERS_DIR"/*.rs 2>/dev/null || true)

# Check for direct neo4rs::Graph imports/usage
echo ""
echo "Scanning for direct neo4rs::Graph imports..."
while IFS=: read -r filepath lineno line; do
    filename=$(basename "$filepath")

    # Get the line above
    prev_lineno=$((lineno - 1))
    prev_line=""
    if [[ $prev_lineno -gt 0 ]]; then
        prev_line=$(sed -n "${prev_lineno}p" "$filepath" 2>/dev/null || true)
    fi

    # Check for exemption
    if [[ "$line" =~ $EXEMPTION_MARKER ]] || [[ "$prev_line" =~ $EXEMPTION_MARKER ]]; then
        echo -e "  ${GREEN}✓${NC} $filename:$lineno (exempted)"
    else
        echo -e "  ${RED}✗${NC} $filename:$lineno - unexempted neo4rs::Graph import/usage"
        echo "$filepath:$lineno: $line" >> "$VIOLATIONS_FILE"
        EXIT_CODE=1
    fi
done < <(grep -rn "neo4rs::Graph" "$HANDLERS_DIR"/*.rs 2>/dev/null || true)

# WorkspaceGraphClient and WorkspaceContext are the approved workspace-scoped patterns — skip them
echo ""
echo "Scanning for unscoped crate::neo4j access..."
while IFS=: read -r filepath lineno line; do
    filename=$(basename "$filepath")

    if [[ "$line" =~ WorkspaceGraphClient ]] || [[ "$line" =~ WorkspaceContext ]]; then
        continue
    fi

    if [[ "$line" =~ execute_neo4j_query ]] || [[ "$line" =~ check_neo4j ]]; then
        continue
    fi

    trimmed_line=$(echo "$line" | sed 's/^[[:space:]]*//')
    if [[ "$trimmed_line" =~ ^// ]]; then
        continue
    fi

    # Get the line above
    prev_lineno=$((lineno - 1))
    prev_line=""
    if [[ $prev_lineno -gt 0 ]]; then
        prev_line=$(sed -n "${prev_lineno}p" "$filepath" 2>/dev/null || true)
    fi

    # Check for exemption
    if [[ "$line" =~ $EXEMPTION_MARKER ]] || [[ "$prev_line" =~ $EXEMPTION_MARKER ]]; then
        echo -e "  ${GREEN}✓${NC} $filename:$lineno (exempted)"
    else
        echo -e "  ${RED}✗${NC} $filename:$lineno - unexempted crate::neo4j access"
        echo "$filepath:$lineno: $line" >> "$VIOLATIONS_FILE"
        EXIT_CODE=1
    fi
done < <(grep -rn "crate::neo4j" "$HANDLERS_DIR"/*.rs 2>/dev/null || true)

# Summary
echo ""
if [[ $EXIT_CODE -eq 0 ]]; then
    echo -e "${GREEN}OK: all handler Neo4j access is properly scoped${NC}"
else
    echo -e "${RED}FAILURE: unexempted Neo4j access detected${NC}"
    echo ""
    echo "Violations:"
    while IFS= read -r violation; do
        echo "  - $violation"
    done < "$VIOLATIONS_FILE"
    echo ""
    echo -e "${YELLOW}Note:${NC} Add '# RUSA-194-EXEMPT: <reason>' comment on the same line or line above to exempt."
fi

exit $EXIT_CODE
