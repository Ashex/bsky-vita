import { mount } from "@pocketjs/framework/solid";
import { Text, View } from "@pocketjs/framework/solid/components";
import { Timeline } from "./Timeline";
declare global {
 var nativeBatch:{commands:Array<Record<string,unknown>>;image:{generation:number;width:number;height:number}|null};
 var __bskyReceiveFeed:(json:string)=>void;var __bskyRequestFeed:(command:string)=>void;var __bskyTakeFeedCommands:()=>string;var __bskySetFeedError:(message:string)=>void;
 var __bskyVisibleCandidates:(json:string)=>string;var __bskyVisibleMeasured:(json:string)=>string;var __bskyVisiblePlacement:string;
}
function App(){return <View class="w-full h-full flex-col bg-slate-950"><Timeline/><Text class="text-xs text-slate-400">Native timeline active</Text></View>;}
globalThis.nativeBatch={commands:[],image:null};globalThis.__bskyVisiblePlacement="";mount(()=> <App/>);
