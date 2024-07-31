import { Await, defer, Link, useLoaderData, useRouteLoaderData } from "react-router-dom";
import PageHeader from "../components/PageHeader";
import type { Account } from "../../../bindings/Account";
import { getAccounts } from "../api";
import AccountsList from "../components/AccountsList";
import { Suspense } from "react";
import LoadingList from "../components/LoadingList";
import Button from "../components/Button";
import { ExchangeRate } from "../../../bindings/ExchangeRate";
import { User } from "../../../bindings/User";
import currencySymbols from "../utils/currency-symbols";

type Data = {
	accounts: Account[],
};

export async function loader() {
	return defer({
		accounts: getAccounts(),
	});
}

export default function Accounts() {
	const { accounts } = useLoaderData() as Data;
	return <>
		<PageHeader>
			<div className="flex space-x-2">
				<span className="flex-1">Accounts</span>
				<Link to="/accounts/new" className="text-xs px-2 bg-purple-100 text-purple-900 font-medium rounded flex items-center space-x-1 active:translate-y-0.5">
					<svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth="2" stroke="currentColor" className="w-4 h-4">
						<path strokeLinecap="round" strokeLinejoin="round" d="M12 9v6m3-3H9m12 0a9 9 0 11-18 0 9 9 0 0118 0z" />
					</svg>
					<span>Add Account</span></Link>
			</div>
		</PageHeader>
		<main className="flex-grow p-4">
			<Suspense fallback={<LoadingList />}>
				<Await resolve={accounts}>
					{(accounts: Account[]) => {
						const { exchangeRates, user } = useRouteLoaderData("app") as { exchangeRates: ExchangeRate, user: User };
						const total_balance = accounts.reduce((total, account) => {
							console.log(exchangeRates.conversion_rates[account.currency], account.balance);
							const primary_currency_balance = exchangeRates.conversion_rates[account.currency] * account.balance;
							console.log(primary_currency_balance)
							if ( isNaN(primary_currency_balance) ) {
								return total;
							}
							total += primary_currency_balance;
							return total;
						}, 0 );
						return (
							<div>
								<div className="flex justify-end">
									<div className="w-24 m-2 text-right sm:text-left">
										<span>Total</span>
										<div className="text-lg">
											{ currencySymbols[ user.primary_currency as keyof typeof currencySymbols ] }
											{ total_balance.toFixed(2) }
										</div>
									</div>
								</div>
								<AccountsList accounts={accounts} />
							</div>
						)
					}}
				</Await>
			</Suspense>
		</main>
	</>
}


