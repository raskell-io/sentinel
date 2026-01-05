#!/bin/bash
# Test script for inline OpenAPI validation

set -e

PROXY_URL="http://localhost:18888"
API_URL="${PROXY_URL}/api/users"

echo "=================================="
echo "Inline OpenAPI Validation Tests"
echo "=================================="
echo ""

# Color codes
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

test_passed=0
test_failed=0

run_test() {
    local name="$1"
    local expected_status="$2"
    shift 2

    echo -e "${YELLOW}TEST:${NC} $name"

    # Run the curl command and capture both status and response
    response=$(curl -s -w "\n%{http_code}" "$@")
    status=$(echo "$response" | tail -n 1)
    body=$(echo "$response" | sed '$d')

    echo "Response: $body"
    echo "Status: $status"

    if [ "$status" = "$expected_status" ]; then
        echo -e "${GREEN}✓ PASS${NC} (expected $expected_status, got $status)"
        ((test_passed++))
    else
        echo -e "${RED}✗ FAIL${NC} (expected $expected_status, got $status)"
        ((test_failed++))
    fi
    echo ""
}

echo "Waiting for services to be ready..."
sleep 2

# Health check
echo -e "${YELLOW}=== Health Check ===${NC}"
curl -s "$PROXY_URL/health" && echo ""
echo ""

# Test 1: Valid user creation (all required fields)
echo -e "${YELLOW}=== Test 1: Valid Request (all required fields) ===${NC}"
run_test "Valid user creation" "201" \
    -X POST "$API_URL" \
    -H "Content-Type: application/json" \
    -d '{
        "email": "alice@example.com",
        "password": "SecurePass123",
        "username": "alice_wonder"
    }'

# Test 2: Valid request with optional field
echo -e "${YELLOW}=== Test 2: Valid Request (with optional age) ===${NC}"
run_test "Valid with optional field" "201" \
    -X POST "$API_URL" \
    -H "Content-Type: application/json" \
    -d '{
        "email": "bob@example.com",
        "password": "MyPassword456",
        "username": "bob_smith",
        "age": 30
    }'

# Test 3: Missing required field (email)
echo -e "${YELLOW}=== Test 3: Missing Required Field (email) ===${NC}"
run_test "Missing email field" "400" \
    -X POST "$API_URL" \
    -H "Content-Type: application/json" \
    -d '{
        "password": "SecurePass123",
        "username": "charlie"
    }'

# Test 4: Missing required field (password)
echo -e "${YELLOW}=== Test 4: Missing Required Field (password) ===${NC}"
run_test "Missing password field" "400" \
    -X POST "$API_URL" \
    -H "Content-Type: application/json" \
    -d '{
        "email": "dave@example.com",
        "username": "dave"
    }'

# Test 5: Invalid email format
echo -e "${YELLOW}=== Test 5: Invalid Email Format ===${NC}"
run_test "Invalid email format" "400" \
    -X POST "$API_URL" \
    -H "Content-Type: application/json" \
    -d '{
        "email": "not-an-email",
        "password": "SecurePass123",
        "username": "eve"
    }'

# Test 6: Password too short (< 8 characters)
echo -e "${YELLOW}=== Test 6: Password Too Short ===${NC}"
run_test "Password too short" "400" \
    -X POST "$API_URL" \
    -H "Content-Type: application/json" \
    -d '{
        "email": "frank@example.com",
        "password": "short",
        "username": "frank"
    }'

# Test 7: Username too short (< 3 characters)
echo -e "${YELLOW}=== Test 7: Username Too Short ===${NC}"
run_test "Username too short" "400" \
    -X POST "$API_URL" \
    -H "Content-Type: application/json" \
    -d '{
        "email": "grace@example.com",
        "password": "SecurePass123",
        "username": "ab"
    }'

# Test 8: Username with invalid characters
echo -e "${YELLOW}=== Test 8: Username Invalid Pattern ===${NC}"
run_test "Username with invalid chars" "400" \
    -X POST "$API_URL" \
    -H "Content-Type: application/json" \
    -d '{
        "email": "henry@example.com",
        "password": "SecurePass123",
        "username": "henry@invalid"
    }'

# Test 9: Age below minimum (< 13)
echo -e "${YELLOW}=== Test 9: Age Below Minimum ===${NC}"
run_test "Age below minimum" "400" \
    -X POST "$API_URL" \
    -H "Content-Type: application/json" \
    -d '{
        "email": "iris@example.com",
        "password": "SecurePass123",
        "username": "iris",
        "age": 10
    }'

# Test 10: Age above maximum (> 120)
echo -e "${YELLOW}=== Test 10: Age Above Maximum ===${NC}"
run_test "Age above maximum" "400" \
    -X POST "$API_URL" \
    -H "Content-Type: application/json" \
    -d '{
        "email": "jack@example.com",
        "password": "SecurePass123",
        "username": "jack",
        "age": 150
    }'

# Test 11: Additional properties (strict mode)
echo -e "${YELLOW}=== Test 11: Additional Properties (strict mode) ===${NC}"
run_test "Additional properties rejected" "400" \
    -X POST "$API_URL" \
    -H "Content-Type: application/json" \
    -d '{
        "email": "kate@example.com",
        "password": "SecurePass123",
        "username": "kate",
        "extra_field": "should_be_rejected"
    }'

# Test 12: GET request (should work without validation)
echo -e "${YELLOW}=== Test 12: GET Request (no validation) ===${NC}"
run_test "GET users list" "200" \
    -X GET "$API_URL"

# Summary
echo "=================================="
echo "Test Summary"
echo "=================================="
echo -e "${GREEN}Passed: $test_passed${NC}"
echo -e "${RED}Failed: $test_failed${NC}"
echo "Total:  $((test_passed + test_failed))"
echo ""

if [ $test_failed -eq 0 ]; then
    echo -e "${GREEN}All tests passed!${NC}"
    exit 0
else
    echo -e "${RED}Some tests failed!${NC}"
    exit 1
fi
