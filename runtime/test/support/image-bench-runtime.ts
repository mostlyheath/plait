// Offline transfer benchmark. No turn is submitted and no model is contacted.
import { appendFileSync } from 'node:fs';
import { join } from 'node:path';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxProvider } from '@earendil-works/pi-ai/providers/faux';
import { Runtime } from '../../src/runtime.ts';
import { serve } from '../../src/stdio.ts';

const models=createModels(); models.setProvider(fauxProvider().provider);
class Fixture extends Runtime {
  override async resources() { return {skills:[],templates:[],problems:[]}; }
}
const runtime=new Fixture(process.argv[2],models,new Set(['faux']));
const read=runtime.store.images.read.bind(runtime.store.images);
let requests=0;
runtime.store.images.read=(thread,id,offset)=>{
  let result;
  if(process.env.PLAIT_TEST_SCENARIO==='old-chunks') {
    // The previous production SQL and bound, with the same host/runtime framing.
    const row=runtime.store.db.prepare('SELECT substr(bytes,?,?) AS chunk FROM images WHERE thread=? AND id=?').get(offset+1,32768,thread,id)!;
    result={image:runtime.store.images.refs(thread,[id])[0],offset,data:Buffer.from(row.chunk as Uint8Array).toString('base64')};
  } else result=read(thread,id,offset);
  requests++;
  if(offset+Buffer.from(result.data,'base64').length===result.image.size) {
    appendFileSync(join(process.argv[2],'image-requests.jsonl'),JSON.stringify(requests)+'\n'); requests=0;
  }
  return result;
};
try { await runtime.recover(); await serve(runtime); } finally { await runtime.close(); }
