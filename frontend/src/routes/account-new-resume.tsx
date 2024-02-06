import { Await, defer, Link, useLoaderData } from "react-router-dom";
import PageHeader from "../components/PageHeader";
import { createAccounts } from "../api";
import { Suspense, useState } from "react";
import LoadingList from "../components/LoadingList";
import AccountsList from "../components/AccountsList";
import { Account } from "../../../bindings/Account";

type Data = {
	accounts: Account[],
};

export async function loader({ request }: { request: Request }) {
	let requisitionId = (new URL(request.url)).searchParams.get('ref');
	if (!requisitionId) {
		throw new Error('Not requisition id found in the URL.');
	}
	return defer({
		accounts: createAccounts(requisitionId),
	})
}

export default function AccountsResume() {
	let { accounts } = useLoaderData() as Data;

	return <>
		<PageHeader>
			<Link to="/accounts">Accounts</Link> &rarr; New Account
		</PageHeader>
		<main className="flex-grow p-4">
			<Suspense
				fallback={<LoadingList number={6} />}
			>
				<Await
					resolve={accounts}
					errorElement={
						<p>Error loading accounts!</p>
					}
				>
					{(accounts: Account[]) => <div>
						<p className="mb-4">The following accounts have been created.</p>
						<AccountsList accounts={accounts} />
					</div>}
				</Await>
			</Suspense>
		</main>
	</>
}


