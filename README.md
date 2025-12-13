# Miden CLOB

![Logo](assets/zkCLOB.png)

# Architecture

### Single fill of a PSWAP note
![single fill](assets/single_fill.png)

The core of the zkCLOB on Miden is the PSWAP note. PSWAP stands for "Partially Fillable SWAP" (as opposed to the standard SWAP note which is not partially fillable).

In the image above, the PSWAP note is filled by a single account.

### Matching two PSWAP notes against each other
![match fill](assets/match_fill.png)

Since it isn't very efficient if every user needs to generate a transaction to fill other user's orders, we can match the PSWAP notes of two traders together. There are 2 possible outcomes:
- Note 1 is partially filled, and note 2 is completely filled.
- Both PSWAP notes are completely filled.

### Batch matching multiple PSWAP notes
![batch fill](assets/batch_fill.png)

To further improve order settlement speed, we can batch match many PSWAP notes at a time.

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