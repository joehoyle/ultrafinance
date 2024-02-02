import { Await, defer, Link, useLoaderData } from "react-router-dom";
import PageHeader from "../components/PageHeader";
import type { Institution } from "../../../bindings/Institution";
import { createRequisition, getInstitutions } from "../api";
import { Suspense, useState } from "react";
import LoadingList from "../components/LoadingList";

type Data = {
	institutions: Institution[],
};

export async function loader() {
	return defer({
		institutions: getInstitutions(),
	})
}

export default function AccountsNew() {
	let { institutions } = useLoaderData() as Data;
	const [isCreatingRequisition, setIsCreatingRequisition] = useState(false);
	const [search, setSearch] = useState("");

	async function onSelectInstitution(institution: Institution) {
		setIsCreatingRequisition(true);
		const requisition = await createRequisition(institution.id);
		window.location.href = requisition.link;
	}

	return <>
		<PageHeader>
			<Link to="/accounts">Accounts</Link> &rarr; New Account
		</PageHeader>
		<main className="flex-grow p-10">
			<div className="flex mb-4">
				<h2 className="flex-1">Select your financial institution</h2>
				<input type="text" onChange={e => setSearch(e.target.value)} placeholder="Search..." className="text-xs text-gray-600 border-gray-300 p-1 border rounded outline-none" />
			</div>
			<div className="relative">
				{isCreatingRequisition && <div className="inset-0 absolute flex items-center justify-center z-10 bg-white/50">
					<svg aria-hidden="true" className="w-8 h-8 mr-2 text-gray-200 animate-spin fill-blue-600" viewBox="0 0 100 101" fill="none" xmlns="http://www.w3.org/2000/svg">
						<path d="M100 50.5908C100 78.2051 77.6142 100.591 50 100.591C22.3858 100.591 0 78.2051 0 50.5908C0 22.9766 22.3858 0.59082 50 0.59082C77.6142 0.59082 100 22.9766 100 50.5908ZM9.08144 50.5908C9.08144 73.1895 27.4013 91.5094 50 91.5094C72.5987 91.5094 90.9186 73.1895 90.9186 50.5908C90.9186 27.9921 72.5987 9.67226 50 9.67226C27.4013 9.67226 9.08144 27.9921 9.08144 50.5908Z" fill="currentColor" />
						<path d="M93.9676 39.0409C96.393 38.4038 97.8624 35.9116 97.0079 33.5539C95.2932 28.8227 92.871 24.3692 89.8167 20.348C85.8452 15.1192 80.8826 10.7238 75.2124 7.41289C69.5422 4.10194 63.2754 1.94025 56.7698 1.05124C51.7666 0.367541 46.6976 0.446843 41.7345 1.27873C39.2613 1.69328 37.813 4.19778 38.4501 6.62326C39.0873 9.04874 41.5694 10.4717 44.0505 10.1071C47.8511 9.54855 51.7191 9.52689 55.5402 10.0491C60.8642 10.7766 65.9928 12.5457 70.6331 15.2552C75.2735 17.9648 79.3347 21.5619 82.5849 25.841C84.9175 28.9121 86.7997 32.2913 88.1811 35.8758C89.083 38.2158 91.5421 39.6781 93.9676 39.0409Z" fill="currentFill" />
					</svg>
				</div>}
				<ul className="flex flex-col space-y-2 h-96 overflow-y-auto">
					<Suspense
						fallback={<LoadingList number={6} />}
					>
						<Await
							resolve={institutions}
							errorElement={
								<p>Error loading institutions!</p>
							}
						>
							{(institutions: Institution[]) => {
								if (search) {
									institutions = institutions.filter(i => i.name.toLocaleLowerCase().indexOf(search.toLocaleLowerCase()) > -1)
								}
								return <>
									{institutions.map(institution => (
										<li className="flex" key={institution.id}>
											<button onClick={() => onSelectInstitution(institution)} className="flex flex-1 space-x-3 py-2 bg-white/50 border-b border-b-slate-50 font-medium text-xs text-slate-600 items-center hover:text-purple-900">
												<img loading="lazy" src={institution.logo} className="w-8 flex-0" />
												<div className="flex-1 text-left flex flex-col">
													<span>{institution.name}</span>
													<span className="font-normal text-gray-400">{institution.transaction_total_days} days of data | Available in {institution.countries.join(', ')}</span>
												</div>
												<svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" strokeWidth={1.5} stroke="currentColor" className="w-4 h-4">
													<path strokeLinecap="round" strokeLinejoin="round" d="M12.75 15l3-3m0 0l-3-3m3 3h-7.5M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
												</svg>
											</button>
										</li>
									))}
								</>
							}}
						</Await>
					</Suspense>

				</ul>
			</div>

		</main>
	</>
}


