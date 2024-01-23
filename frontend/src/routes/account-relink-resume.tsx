import { Await, defer, Link, Params, useLoaderData } from "react-router-dom";
import PageHeader from "../components/PageHeader";
import { relinkAccount } from "../api";
import { Suspense } from "react";
import LoadingList from "../components/LoadingList";
import { Account } from "../../../bindings/Account";

type Data = {
	account: Account,
};

type RouteParams = Params;

export async function loader({ request, params }: { request: Request, params: RouteParams }) {
	let requisitionId = (new URL(request.url)).searchParams.get('ref');
	if (!requisitionId) {
		throw new Error('Not requisition id found in the URL.');
	}
	return defer({
		account: relinkAccount(Number(params.id), requisitionId),
	})
}

export default function AccountRelinkResume() {
	let { account } = useLoaderData() as Data;

	return <>
		<PageHeader>
			<Link to="/accounts">Accounts</Link> &rarr; Relink
		</PageHeader>
		<main className="flex-grow p-10">
			<Suspense
				fallback={<LoadingList number={6} />}
			>
				<Await
					resolve={account}
					errorElement={
						<p>Error relinking account!</p>
					}
				>
					{ (account: Account) => <p>Relinked account {account.name }.</p> }
				</Await>
			</Suspense>
		</main>
	</>
}


