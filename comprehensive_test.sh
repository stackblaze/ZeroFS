#!/bin/bash

# ZeroFS COW Clone Test - Simple and Focused
# Tests: Clone files and directories, verify COW independence

MOUNT_POINT="/tmp/zerofs-test-mount"
ZEROFS_CLI="./zerofs/target/release/zerofs"
CONFIG="zerofs.toml"

# Color codes
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

print_header() {
    echo ""
    echo "==================================================================="
    echo -e "${BLUE}  $1${NC}"
    echo "==================================================================="
    echo ""
}

print_step() {
    echo ""
    echo "-------------------------------------------------------------------"
    echo -e "${YELLOW}$1${NC}"
    echo "-------------------------------------------------------------------"
}

print_success() {
    echo -e "${GREEN}✓ $1${NC}"
}

print_error() {
    echo -e "${RED}✗ $1${NC}"
}

step1_mount() {
    print_step "Step 1: Mount ZeroFS"
    
    mkdir -p $MOUNT_POINT
    
    if mountpoint -q $MOUNT_POINT 2>/dev/null; then
        print_success "Already mounted at $MOUNT_POINT"
        return 0
    fi
    
    if sudo mount -t 9p -o trans=tcp,port=5564 127.0.0.1 $MOUNT_POINT 2>/dev/null; then
        print_success "Mounted at $MOUNT_POINT"
    else
        print_error "Mount failed. Is ZeroFS server running?"
        return 1
    fi
}

step2_test_file_clone() {
    print_step "Step 2: Test File Clone (COW)"
    
    if ! mountpoint -q $MOUNT_POINT 2>/dev/null; then
        print_error "Not mounted. Run step 1 first."
        return 1
    fi
    
    # Clean up
    sudo rm -f $MOUNT_POINT/original-file.txt $MOUNT_POINT/cloned-file.txt 2>/dev/null || true
    sync && sleep 1
    
    # Create original file
    echo "Original content - $(date)" | sudo tee $MOUNT_POINT/original-file.txt > /dev/null
    sync && sleep 1
    
    echo "Created original file:"
    cat $MOUNT_POINT/original-file.txt
    echo ""
    
    # Clone the file
    echo "Cloning file..."
    if ! $ZEROFS_CLI dataset clone -c $CONFIG \
        --source /original-file.txt \
        --destination /cloned-file.txt 2>&1; then
        print_error "File clone failed"
        return 1
    fi
    
    sync && sleep 1
    print_success "File cloned"
    echo ""
    
    # Verify clone exists and has same content
    echo "Verifying clone has same content:"
    if [ ! -f "$MOUNT_POINT/cloned-file.txt" ]; then
        print_error "Cloned file doesn't exist"
        return 1
    fi
    
    cat $MOUNT_POINT/cloned-file.txt
    echo ""
    
    # Modify the clone
    echo "Modifying cloned file..."
    echo "MODIFIED CLONE - $(date)" | sudo tee $MOUNT_POINT/cloned-file.txt > /dev/null
    sync && sleep 1
    
    # Verify original is unchanged (COW independence)
    echo ""
    echo "Checking if original is unchanged (COW test):"
    ORIGINAL_CONTENT=$(cat $MOUNT_POINT/original-file.txt)
    CLONED_CONTENT=$(cat $MOUNT_POINT/cloned-file.txt)
    
    echo "Original: $ORIGINAL_CONTENT"
    echo "Clone:    $CLONED_CONTENT"
    echo ""
    
    if [[ "$ORIGINAL_CONTENT" == *"Original content"* ]] && [[ "$CLONED_CONTENT" == *"MODIFIED CLONE"* ]]; then
        print_success "COW works! Original unchanged, clone modified independently"
    else
        print_error "COW failed! Files are not independent"
        return 1
    fi
}

step3_test_directory_clone() {
    print_step "Step 3: Test Directory Clone (COW)"
    
    if ! mountpoint -q $MOUNT_POINT 2>/dev/null; then
        print_error "Not mounted. Run step 1 first."
        return 1
    fi
    
    # Clean up
    sudo rm -rf $MOUNT_POINT/original-dir $MOUNT_POINT/cloned-dir 2>/dev/null || true
    sync && sleep 1
    
    # Create original directory with files
    echo "Creating original directory structure..."
    sudo mkdir -p $MOUNT_POINT/original-dir/subdir
    echo "File A - $(date)" | sudo tee $MOUNT_POINT/original-dir/fileA.txt > /dev/null
    echo "File B - $(date)" | sudo tee $MOUNT_POINT/original-dir/fileB.txt > /dev/null
    echo "File C in subdir - $(date)" | sudo tee $MOUNT_POINT/original-dir/subdir/fileC.txt > /dev/null
    sync && sleep 1
    
    echo "Original directory structure:"
    find $MOUNT_POINT/original-dir -type f | sort
    echo ""
    
    # Clone the directory
    echo "Cloning directory..."
    if ! $ZEROFS_CLI dataset clone -c $CONFIG \
        --source /original-dir \
        --destination /cloned-dir 2>&1; then
        print_error "Directory clone failed"
        return 1
    fi
    
    sync && sleep 1
    print_success "Directory cloned"
    echo ""
    
    # Verify clone structure
    echo "Verifying cloned directory structure:"
    if [ ! -d "$MOUNT_POINT/cloned-dir" ]; then
        print_error "Cloned directory doesn't exist"
        return 1
    fi
    
    CLONE_FILES=$(find $MOUNT_POINT/cloned-dir -type f 2>/dev/null | wc -l)
    if [ "$CLONE_FILES" -ne 3 ]; then
        print_error "Cloned directory has $CLONE_FILES files, expected 3"
        return 1
    fi
    
    find $MOUNT_POINT/cloned-dir -type f | sort
    print_success "All files present in clone"
    echo ""
    
    # Verify file contents match
    echo "Verifying file contents match:"
    ORIG_A=$(cat $MOUNT_POINT/original-dir/fileA.txt)
    CLONE_A=$(cat $MOUNT_POINT/cloned-dir/fileA.txt)
    
    if [ "$ORIG_A" = "$CLONE_A" ]; then
        print_success "File contents match"
    else
        print_error "File contents don't match"
        return 1
    fi
    echo ""
    
    # Modify a file in the clone
    echo "Modifying file in cloned directory..."
    echo "MODIFIED IN CLONE - $(date)" | sudo tee $MOUNT_POINT/cloned-dir/fileA.txt > /dev/null
    sync && sleep 1
    
    # Verify original is unchanged
    echo ""
    echo "Checking if original is unchanged (COW test):"
    ORIG_A_AFTER=$(cat $MOUNT_POINT/original-dir/fileA.txt)
    CLONE_A_AFTER=$(cat $MOUNT_POINT/cloned-dir/fileA.txt)
    
    echo "Original: $ORIG_A_AFTER"
    echo "Clone:    $CLONE_A_AFTER"
    echo ""
    
    if [[ "$ORIG_A_AFTER" == *"File A"* ]] && [[ "$CLONE_A_AFTER" == *"MODIFIED IN CLONE"* ]]; then
        print_success "COW works! Original unchanged, clone modified independently"
    else
        print_error "COW failed! Files are not independent"
        return 1
    fi
    
    # Add a new file to clone
    echo ""
    echo "Adding new file to cloned directory..."
    echo "New file in clone" | sudo tee $MOUNT_POINT/cloned-dir/newfile.txt > /dev/null
    sync && sleep 1
    
    # Verify original doesn't have the new file
    if [ ! -f "$MOUNT_POINT/cloned-dir/newfile.txt" ]; then
        print_error "New file wasn't created in clone"
        return 1
    fi
    
    if [ -f "$MOUNT_POINT/original-dir/newfile.txt" ]; then
        print_error "New file appeared in original (not independent!)"
        return 1
    fi
    
    print_success "New file only in clone, not in original - directories are independent"
}

step4_test_nested_directory_clone() {
    print_step "Step 4: Test Nested Directory Clone"
    
    if ! mountpoint -q $MOUNT_POINT 2>/dev/null; then
        print_error "Not mounted. Run step 1 first."
        return 1
    fi
    
    # Clean up
    sudo rm -rf $MOUNT_POINT/deep-dir $MOUNT_POINT/deep-clone 2>/dev/null || true
    sync && sleep 1
    
    # Create deeply nested structure
    echo "Creating deeply nested directory..."
    sudo mkdir -p $MOUNT_POINT/deep-dir/level1/level2/level3
    echo "Deep file" | sudo tee $MOUNT_POINT/deep-dir/level1/level2/level3/deep.txt > /dev/null
    echo "Root file" | sudo tee $MOUNT_POINT/deep-dir/root.txt > /dev/null
    sync && sleep 1
    
    # Clone it
    echo "Cloning nested directory..."
    if ! $ZEROFS_CLI dataset clone -c $CONFIG \
        --source /deep-dir \
        --destination /deep-clone 2>&1; then
        print_error "Nested directory clone failed"
        return 1
    fi
    
    sync && sleep 1
    
    # Verify deep file exists
    if [ -f "$MOUNT_POINT/deep-clone/level1/level2/level3/deep.txt" ]; then
        print_success "Deeply nested file cloned correctly"
        cat $MOUNT_POINT/deep-clone/level1/level2/level3/deep.txt
    else
        print_error "Deeply nested file not found in clone"
        return 1
    fi
}

step5_test_rest_api_clone() {
    print_step "Step 5: Test Clone via REST API"
    
    if ! mountpoint -q $MOUNT_POINT 2>/dev/null; then
        print_error "Not mounted. Run step 1 first."
        return 1
    fi
    
    # Clean up
    sudo rm -rf $MOUNT_POINT/api-test-dir $MOUNT_POINT/api-clone 2>/dev/null || true
    sync && sleep 1
    
    # Create test directory
    sudo mkdir -p $MOUNT_POINT/api-test-dir
    echo "API test file" | sudo tee $MOUNT_POINT/api-test-dir/test.txt > /dev/null
    sync && sleep 1
    
    echo "Testing REST API clone..."
    RESPONSE=$(curl -s -X POST http://127.0.0.1:8080/api/v1/clone \
        -H "Content-Type: application/json" \
        -d '{
            "source": "/api-test-dir",
            "destination": "/api-clone"
        }')
    
    echo "$RESPONSE" | jq . 2>/dev/null || echo "$RESPONSE"
    
    if echo "$RESPONSE" | grep -q "cloned successfully"; then
        print_success "REST API clone succeeded"
        
        # Verify clone exists
        sync && sleep 1
        if [ -f "$MOUNT_POINT/api-clone/test.txt" ]; then
            print_success "Cloned file accessible"
            cat $MOUNT_POINT/api-clone/test.txt
        else
            print_error "Cloned file not found"
            return 1
        fi
    else
        print_error "REST API clone failed"
        return 1
    fi
}

show_summary() {
    print_header "Test Results Summary"
    
    echo "✅ File clone: Works with COW"
    echo "✅ Directory clone: Works with COW"
    echo "✅ Nested directory clone: Works"
    echo "✅ REST API clone: Works"
    echo "✅ COW independence: Verified (modifications don't affect source)"
    echo ""
    echo "Key Findings:"
    echo "  • Clone creates instant COW copies"
    echo "  • Source and clone are independent"
    echo "  • Modifying clone doesn't affect source"
    echo "  • Adding files to clone doesn't affect source"
    echo "  • Works for files, directories, and nested structures"
    echo "  • Works via CLI and REST API"
    echo ""
    echo -e "${GREEN}All tests passed! Clone works perfectly.${NC}"
    echo ""
}

cleanup() {
    print_step "Cleanup"
    if mountpoint -q $MOUNT_POINT 2>/dev/null; then
        sudo rm -rf $MOUNT_POINT/original-* $MOUNT_POINT/cloned-* \
                    $MOUNT_POINT/deep-* $MOUNT_POINT/api-* 2>/dev/null || true
        print_success "Test files cleaned up"
    fi
}

show_usage() {
    echo "Usage: $0 [step_number|all|cleanup]"
    echo ""
    echo "Steps:"
    echo "  1  - Mount ZeroFS"
    echo "  2  - Test file clone (with modification)"
    echo "  3  - Test directory clone (with modification)"
    echo "  4  - Test nested directory clone"
    echo "  5  - Test REST API clone"
    echo "  all - Run all steps (default)"
    echo "  cleanup - Clean up test files"
    echo ""
    echo "Examples:"
    echo "  $0           # Run all tests"
    echo "  $0 2         # Test file clone only"
    echo "  $0 3         # Test directory clone only"
    echo ""
}

# Main execution
main() {
    local step="${1:-all}"
    
    print_header "ZeroFS Clone Test - COW Verification"
    
    case "$step" in
        1)
            step1_mount
            ;;
        2)
            step2_test_file_clone
            ;;
        3)
            step3_test_directory_clone
            ;;
        4)
            step4_test_nested_directory_clone
            ;;
        5)
            step5_test_rest_api_clone
            ;;
        cleanup)
            cleanup
            ;;
        all)
            cleanup  # Clean up first
            step1_mount && \
            step2_test_file_clone && \
            step3_test_directory_clone && \
            step4_test_nested_directory_clone && \
            step5_test_rest_api_clone && \
            show_summary
            ;;
        help|--help|-h)
            show_usage
            ;;
        *)
            echo "Error: Unknown step '$step'"
            echo ""
            show_usage
            exit 1
            ;;
    esac
}

# Run main with all arguments
main "$@"
