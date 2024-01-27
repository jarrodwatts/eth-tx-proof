use std::sync::Condvar;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Result};
use ops::{AggProof, AggregatableProofWithIdentity, BlockProof, TxProof};
use paladin::{
    directive::{Directive, IndexedStream, Literal},
    operation::{Monoid, Operation},
    runtime::Runtime,
};
use plonky_block_proof_gen::{
    proof_types::{AggregatableProof, GeneratedBlockProof},
    types::PlonkyProofIntern,
};
use protocol_decoder::types::TxnProofGenIR;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::{info_span, Level};

#[derive(Debug, Deserialize, Serialize)]
pub struct ProverInput {
    pub proof_gen_ir: Vec<TxnProofGenIR>,
}

impl ProverInput {
    pub async fn prove(
        self,
        runtime: &Runtime,
        _previous: Option<PlonkyProofIntern>,
    ) -> Result<GeneratedBlockProof> {
        tracing::info!("Proving block");
        let agg_proof = IndexedStream::from(self.proof_gen_ir)
            .map(&TxProof)
            .fold(&AggProof)
            .run(runtime)
            .await?;

        if let AggregatableProofWithIdentity::Agg(AggregatableProof::Agg(proof)) = agg_proof {
            let block_proof = Literal(proof)
                .map(&BlockProof { prev: None })
                .run(runtime)
                .await?;
            tracing::info!("Block proof generated");

            Ok(block_proof.0)
        } else {
            bail!("AggProof is is not GeneratedAggProof")
        }
    }

    pub fn prove_in_memory(self, paralellism: usize) -> Result<GeneratedBlockProof> {
        let span = info_span!("generate tx proofs").entered();
        let start = std::time::Instant::now();
        tracing::event!(Level::INFO, "generating tx proofs");
        let counter = Arc::new((Mutex::new(0), Condvar::new()));

        let txs = self.proof_gen_ir.into_par_iter().map(|tx| {
            let (lock, cvar) = &*counter;
            let mut count = lock.lock().unwrap();
            while *count >= paralellism {
                count = cvar.wait(count).unwrap();
            }
            *count += 1;
            drop(count);

            let proof = TxProof.execute(tx).unwrap();
            let mut count = lock.lock().unwrap();
            *count -= 1;
            cvar.notify_all();
            proof
        });
        tracing::event!(Level::INFO, "generate tx proofs took {:?}", start.elapsed());
        span.exit();

        let span = info_span!("aggregate proofs").entered();
        let start = std::time::Instant::now();
        tracing::event!(Level::INFO, "aggregating proofs");
        let agg_proof = txs.reduce(
            || AggregatableProofWithIdentity::Unit,
            |a, b| AggProof.combine(a, b).unwrap(),
        );
        tracing::event!(Level::INFO, "aggregate proofs took {:?}", start.elapsed());
        span.exit();

        if let AggregatableProofWithIdentity::Agg(AggregatableProof::Agg(proof)) = agg_proof {
            let span = info_span!("generate block proof").entered();
            let start = std::time::Instant::now();
            tracing::event!(Level::INFO, "generating block proof");
            let b_proof = BlockProof { prev: None };
            let block_proof = b_proof.execute(proof).unwrap();
            tracing::event!(
                Level::INFO,
                "generate block proof took {:?}",
                start.elapsed()
            );
            span.exit();

            Ok(block_proof)
        } else {
            bail!("AggProof is is not GeneratedAggProof")
        }
    }
}
