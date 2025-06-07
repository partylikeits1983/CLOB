# Miden CLOB

![Logo](assets/zkCLOB.png)


## Running the demo:
```
./run_demo.sh
```

## Running the Depth Chart CLI tool:
```
cargo run --bin depth_chart
```

## Running tests:

Running all tests:
```
cargo test --release -- --test-threads=1
```

Running tests that don't use the client: 
```
cargo test -- --ignored
```

Running specific test
```
cargo test --release  swap_note_edge_case_test -- --exact --nocapture
```



#### Note:
This is a WIP / Expiremental project