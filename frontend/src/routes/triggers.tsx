import { Await, defer, Link, useLoaderData } from "react-router-dom";
import PageHeader from "../components/PageHeader";
import type { Trigger } from "../../../bindings/Trigger";
import { getTriggers } from "../api";
import TriggersList from "../components/TriggersList";
import { Suspense } from "react";
import LoadingList from "../components/LoadingList";

type Data = {
	triggers: Trigger[],
};

export async function loader() {
	return defer({
		triggers: getTriggers(),
	});
}

export default function Triggers() {
	const { triggers } = useLoaderData() as Data;
	return <>
		<PageHeader>
			<div className="flex">
				<span className="flex-1">Triggers</span>
				<Link to="/triggers/new" className="text-xs px-2 bg-purple-100 text-purple-900 font-medium rounded flex items-center space-x-1 active:translate-y-0.5">
					<svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth="2" stroke="currentColor" className="w-4 h-4">
						<path strokeLinecap="round" strokeLinejoin="round" d="M12 9v6m3-3H9m12 0a9 9 0 11-18 0 9 9 0 0118 0z" />
					</svg>
					<span>Add Trigger</span></Link>
			</div>
		</PageHeader>
		<main className="flex-grow p-10">
			<Suspense fallback={<LoadingList />}>
				<Await resolve={ triggers }>
					{ triggers => <TriggersList triggers={triggers} /> }
				</Await>
			</Suspense>
		</main>
	</>
}
