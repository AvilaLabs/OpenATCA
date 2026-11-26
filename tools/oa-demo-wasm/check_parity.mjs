// REQ: OA-DEMO-001 — exercise the actual browser module, then replay the same inputs natively.
import assert from 'node:assert/strict';
import {readFile, writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import path from 'node:path';

const [wasmPath,nativePath,assetPath,source,out] = process.argv.slice(2);
assert.equal(process.argv.length,7);
assert.match(source,/^[0-9a-f]{40}$/);
const hash=b=>createHash('sha256').update(b).digest('hex');
const wasm=await readFile(wasmPath), asset=await readFile(assetPath);
const manifest={schema:'oa-f2-wasm-build-v1',build_id:hash(wasm),wasm_sha256:hash(wasm),source_commit:source,scenario_id:'ord-normal-push-v1',scenario_seed:'0x4F52442D4E505631',scenario_asset_sha256:hash(asset)};
const {instance}=await WebAssembly.instantiate(wasm,{});const e=instance.exports;
assert.equal(e.f2_reset(0x4F52442D4E505631n),0,'actual WASM startup must succeed');
const frame=()=>{
 const range=BigInt.asUintN(64,e.f2_frame()),offset=Number(range>>32n),length=Number(range&0xffffffffn);
 assert.equal(length,3686424);assert(offset+length<=e.memory.buffer.byteLength);
 return hash(new Uint8Array(e.memory.buffer,offset,length));
};
const trace=['F2TRACE1',`BUILD\t${manifest.build_id}`,`SOURCE\t${source}`,`ASSET\t${manifest.scenario_asset_sha256}`,'SCENARIO\tord-normal-push-v1','SEED\t'+manifest.scenario_seed,'FRAME\t0\t'+frame()];
const events=[['A',60000],['K',0x2d,0,1,0],['K',0x2d,0,0,0],['K',0x2e,0,1,0],['K',0x4a,0,1,0],['A',300000],['A',900000]];
for (const [i,event] of events.entries()) {
 const status=event[0]==='A'?e.f2_advance_to(BigInt(event[1])):e.f2_key(...event.slice(1));
 assert.equal(status,0,'scripted supported input must succeed');
 trace.push(event.join('\t'),'RESULT\t'+status,'FRAME\t'+(i+1)+'\t'+frame());
}
const manifestPath=path.join(out,'build.json'),tracePath=path.join(out,'smoke.f2trace');
await writeFile(manifestPath,JSON.stringify(manifest)+'\n',{flag:'wx'});
await writeFile(tracePath,trace.join('\n')+'\n',{flag:'wx'});
const nativeOut=path.join(out,'native.f2result'),wasmOut=path.join(out,'wasm.f2result');
for(const [command,args] of [
 [nativePath,['--scenario',assetPath,'--trace',tracePath,'--source-commit',source,'--out',nativeOut]],
 [process.execPath,[fileURLToPath(new URL('./replay_trace.mjs',import.meta.url)),'--wasm',wasmPath,'--manifest',manifestPath,'--scenario',assetPath,'--trace',tracePath,'--out',wasmOut]],
]) {
 const result=spawnSync(command,args,{timeout:120000,maxBuffer:1024*1024,encoding:'utf8'});
 if(result.error)throw result.error;
 assert.equal(result.status,0,`${command}: ${result.stderr}`);
}
const nativeBytes=await readFile(nativeOut),wasmBytes=await readFile(wasmOut);
assert(nativeBytes.equals(wasmBytes),'native and actual WASM frame results differ');
await writeFile(path.join(out,'result.json'),JSON.stringify({source_commit:source,events:events.length,wasm_startup:true,byte_identical:true,result_sha256:hash(nativeBytes)},null,2)+'\n',{flag:'wx'});
console.log(`F2 actual WASM startup and native parity PASS (${events.length} events)`);
