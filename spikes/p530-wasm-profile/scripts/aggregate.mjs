import fs from 'fs';
const dir=process.argv[2];
const med=a=>{a=a.filter(x=>x!=null).sort((x,y)=>x-y);return a.length?a[Math.floor(a.length/2)]:null};
const files=new Set();for(const r of ['r1','r2','r3'])for(const f of fs.readdirSync(`${dir}/${r}`))if(f.endsWith('.json'))files.add(f);
const stages=['copyIn','parse','rebuild','extract','encode','copyOut','jsParse','drop','total','wasmStagesTotal','allOneCall','engineBinding'];
for(const f of [...files].sort()){
 const rs=['r1','r2','r3'].map(r=>{try{return JSON.parse(fs.readFileSync(`${dir}/${r}/${f}`))}catch{return null}}).filter(Boolean);
 if(rs[0].results){console.log(f, rs[0].results.map((x,i)=>x.set+'='+(med(rs.map(r=>r.results[i].medianUs))/1000).toFixed(2)+'ms').join(' '));continue}
 const out=[];for(const s of stages){const v=med(rs.map(r=>r[s]?.medianUs));if(v!=null)out.push(`${s}=${(v/1000).toFixed(2)}`)}
 const m=['microAllocNsPerRound','microSipHashNsPerKey','microMemcpyNsPer4KiB'].map(k=>k.slice(5,10)+'='+med(rs.map(r=>r[k])).toFixed(1));
 console.log(f.replace('.json',''),`(n=${rs.length})`,out.join(' '),m.join(' '));
}
